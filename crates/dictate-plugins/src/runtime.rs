//! Plugin code runs in coroutines the host drives.
//!
//! Every plugin call (a file's top level, a transform, a hook) runs in its
//! own coroutine with a budget of Lua CPU time. A core call such as
//! `dictate.exec` parks the coroutine instead of blocking the host: it
//! yields a [`PendingCall`] that plugin code cannot construct, and the host
//! resumes the coroutine with the result. Time parked on a call does not
//! count against the budget.
//!
//! The instruction hook stops a coroutine that outruns its budget by
//! suspending it; the host never resumes it again. A suspension, unlike an
//! error, cannot be caught by the plugin's own `pcall`, so a runaway call
//! always ends. Time inside a single C function, such as one huge
//! `string.rep`, is not interrupted.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use mlua::Function;
use mlua::HookTriggers;
use mlua::IntoLuaMulti;
use mlua::Lua;
use mlua::MultiValue;
use mlua::Thread;
use mlua::UserData;
use mlua::Value;
use mlua::VmState;
use mlua::thread::ThreadStatus;

use crate::TranscriptContext;
use crate::core::CoreCall;
use crate::core::CoreResult;
use crate::exec::ExecRequest;

/// How often, in VM instructions, the budget is checked.
const BUDGET_CHECK_INSTRUCTIONS: u32 = 1_000;

/// A core call a coroutine is parked on.
pub(crate) enum Call {
    /// Run a command on its own thread.
    Exec(ExecRequest),
    /// Ask the daemon; the host resumes the coroutine with a [`CoreReplyValue`].
    Core(CoreCall),
    /// Run the transcript hook chain; the host resumes the coroutine with
    /// the resulting text.
    Chain {
        text: String,
        context: TranscriptContext,
    },
}

/// The daemon's answer, handed to the parked API function.
pub(crate) struct CoreReplyValue(pub(crate) CoreResult);

impl UserData for CoreReplyValue {}

/// Park the calling coroutine on a daemon call and return the daemon's
/// answer.
pub(crate) async fn core(lua: &Lua, name: &str, call: CoreCall) -> mlua::Result<CoreResult> {
    let values = park(lua, name, Call::Core(call)).await?;
    let (1, Some(Value::UserData(data))) = (values.len(), values.front()) else {
        return Err(mlua::Error::runtime(format!(
            "{name} was resumed without a reply"
        )));
    };
    let reply = data.take::<CoreReplyValue>()?;
    Ok(reply.0)
}

/// What a coroutine yields to park on a call. Only Rust creates it, so a
/// plugin's own `coroutine.yield` is never mistaken for a call.
struct PendingCall(RefCell<Option<Call>>);

impl UserData for PendingCall {}

/// The coroutines the host is driving, so a call made from a coroutine the
/// plugin created itself is refused instead of yielding to the plugin.
#[derive(Default)]
struct Tasks(RefCell<HashSet<*const c_void>>);

/// The plugin whose code is running, if any.
struct ActivePlugin(RefCell<Option<String>>);

pub(crate) fn install(lua: &Lua) {
    lua.set_app_data(Tasks::default());
    lua.set_app_data(ActivePlugin(RefCell::new(None)));
}

pub(crate) fn active_plugin(lua: &Lua) -> Option<String> {
    lua.app_data_ref::<ActivePlugin>()
        .and_then(|active| active.0.borrow().clone())
}

/// Park the calling coroutine on `call` and return what the host resumes it
/// with. For use inside async API functions.
pub(crate) async fn park(lua: &Lua, name: &str, call: Call) -> mlua::Result<MultiValue> {
    let current = lua.current_thread().to_pointer();
    let driven = lua
        .app_data_ref::<Tasks>()
        .is_some_and(|tasks| tasks.0.borrow().contains(&current));
    if !driven {
        return Err(mlua::Error::runtime(format!(
            "{name} must be called from the plugin's own code, not from a coroutine the plugin created"
        )));
    }
    lua.yield_with::<MultiValue>(PendingCall(RefCell::new(Some(call))))
        .await
}

/// What one resume of a task came to.
pub(crate) enum Step {
    Done(MultiValue),
    Parked(Call),
    Failed(mlua::Error),
}

/// One plugin call running in its own coroutine.
pub(crate) struct Task {
    lua: Lua,
    thread: Thread,
    plugin: String,
    budget: Duration,
    /// Lua time left; time parked on calls is not spent.
    remaining: Duration,
    deadline: Rc<Cell<Option<Instant>>>,
    expired: Rc<Cell<bool>>,
}

impl Task {
    pub(crate) fn new(
        lua: &Lua,
        plugin: &str,
        budget: Duration,
        function: &Function,
    ) -> mlua::Result<Self> {
        let thread = lua.create_thread(function.clone())?;
        let deadline = Rc::new(Cell::new(None::<Instant>));
        let expired = Rc::new(Cell::new(false));
        let (hook_deadline, hook_expired) = (Rc::clone(&deadline), Rc::clone(&expired));
        thread.set_hook(
            HookTriggers::new().every_nth_instruction(BUDGET_CHECK_INSTRUCTIONS),
            move |_, _| {
                if hook_deadline
                    .get()
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    hook_expired.set(true);
                    return Ok(VmState::Yield);
                }
                Ok(VmState::Continue)
            },
        )?;
        if let Some(tasks) = lua.app_data_ref::<Tasks>() {
            tasks.0.borrow_mut().insert(thread.to_pointer());
        }
        Ok(Self {
            lua: lua.clone(),
            thread,
            plugin: plugin.to_owned(),
            budget,
            remaining: budget,
            deadline,
            expired,
        })
    }

    pub(crate) fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Run until the task finishes, fails, or parks on a call.
    pub(crate) fn resume(&mut self, lua: &Lua, args: impl IntoLuaMulti) -> Step {
        set_active(lua, Some(&self.plugin));
        let started = Instant::now();
        self.deadline.set(Some(started + self.remaining));
        let result = self.thread.resume::<MultiValue>(args);
        self.deadline.set(None);
        self.remaining = self.remaining.saturating_sub(started.elapsed());
        set_active(lua, None);

        match result {
            Err(error) => Step::Failed(error),
            Ok(values) if self.thread.status() != ThreadStatus::Resumable => Step::Done(values),
            Ok(_) if self.expired.get() => Step::Failed(mlua::Error::runtime(format!(
                "stopped after exceeding its {} ms time budget",
                self.budget.as_millis()
            ))),
            Ok(values) => match pending_call(&values) {
                Some(call) => Step::Parked(call),
                None => Step::Failed(mlua::Error::runtime(
                    "yielded its handler's coroutine, which plugin code must not do",
                )),
            },
        }
    }

    /// Run to the end, resolving calls on this thread. For plugin code that
    /// runs before the host serves events: loading files and transforms.
    pub(crate) fn run_blocking(
        mut self,
        lua: &Lua,
        args: impl IntoLuaMulti,
    ) -> mlua::Result<MultiValue> {
        let mut step = self.resume(lua, args);
        loop {
            match step {
                Step::Done(values) => return Ok(values),
                Step::Failed(error) => return Err(error),
                Step::Parked(Call::Exec(request)) => {
                    let outcome = crate::exec::run(&request);
                    step = self.resume(lua, crate::exec::into_lua(lua, outcome)?);
                }
                Step::Parked(Call::Core(_) | Call::Chain { .. }) => {
                    let refused = CoreReplyValue(Err(
                        "this call only works in event handlers and transcript hooks, not while plugins load".to_owned(),
                    ));
                    step = self.resume(lua, lua.create_userdata(refused)?);
                }
            }
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(tasks) = self.lua.app_data_ref::<Tasks>() {
            tasks.0.borrow_mut().remove(&self.thread.to_pointer());
        }
    }
}

fn pending_call(values: &MultiValue) -> Option<Call> {
    let (1, Some(Value::UserData(data))) = (values.len(), values.front()) else {
        return None;
    };
    let pending = data.borrow::<PendingCall>().ok()?;
    pending.0.borrow_mut().take()
}

fn set_active(lua: &Lua, plugin: Option<&str>) {
    if let Some(active) = lua.app_data_ref::<ActivePlugin>() {
        *active.0.borrow_mut() = plugin.map(str::to_owned);
    }
}
