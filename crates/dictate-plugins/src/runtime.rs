//! Plugin code runs in coroutines the host drives.
//!
//! Every plugin call (a file's top level, a transform, a hook) runs in its
//! own coroutine. A core call such as `dictate.exec` parks the coroutine
//! instead of blocking the host: it yields a [`PendingCall`] that plugin
//! code cannot construct, and the host resumes the coroutine with the
//! result. Plugin code that never returns or parks holds the plugin thread;
//! like `OpenCode`, Dictate does not time plugins out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use mlua::Function;
use mlua::IntoLuaMulti;
use mlua::Lua;
use mlua::MultiValue;
use mlua::Table;
use mlua::Thread;
use mlua::UserData;
use mlua::Value;
use mlua::thread::ThreadStatus;

use crate::core::CoreCall;
use crate::core::CoreResult;
use crate::exec::ExecRequest;

/// A core call a coroutine is parked on.
pub(crate) enum Call {
    /// Run a command on its own thread.
    Exec(ExecRequest),
    /// Ask the daemon; the host resumes the coroutine with a [`CoreReplyValue`].
    Core(CoreCall),
    /// Run the hooks registered under `key` on `event`; the host resumes
    /// the coroutine with the event, or a [`CoreReplyValue`] error naming
    /// the hook that failed.
    Hooks { key: &'static str, event: Table },
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

/// Park the calling coroutine while the hooks under `key` run on `event`,
/// in plugin order, each seeing the changes of the ones before. Fails with
/// the first hook's error.
pub(crate) async fn hooks(
    lua: &Lua,
    name: &str,
    key: &'static str,
    event: Table,
) -> mlua::Result<Table> {
    let values = park(lua, name, Call::Hooks { key, event }).await?;
    match values.front() {
        Some(Value::Table(event)) => Ok(event.clone()),
        Some(Value::UserData(data)) => match data.take::<CoreReplyValue>()?.0 {
            Err(failure) => Err(mlua::Error::runtime(failure)),
            Ok(_) => Err(mlua::Error::runtime(format!(
                "{name} was resumed without its event"
            ))),
        },
        _ => Err(mlua::Error::runtime(format!(
            "{name} was resumed without its event"
        ))),
    }
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

/// What each parked coroutine is resumed with, handed over beside the
/// resume rather than through it: in mlua 0.12.2, when one coroutine
/// catches a Rust callback's error while another waits in `yield_with`,
/// the waiting one is resumed with a stray value in front of its own.
#[derive(Default)]
struct Resumed(RefCell<HashMap<*const c_void, MultiValue>>);

/// Temporary files each coroutine made, deleted when it ends.
#[derive(Default)]
struct TempFiles(RefCell<HashMap<*const c_void, Vec<PathBuf>>>);

/// A new path for a temporary file named `*.{extension}`, deleted when the
/// calling plugin call ends. It sits in a directory only this user can
/// read, since it may hold a recording.
pub(crate) fn temp_file(lua: &Lua, name: &str, extension: &str) -> mlua::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let current = lua.current_thread().to_pointer();
    let driven = lua
        .app_data_ref::<Tasks>()
        .is_some_and(|tasks| tasks.0.borrow().contains(&current));
    if !driven {
        return Err(mlua::Error::runtime(format!(
            "{name} must be called from the plugin's own code, not from a coroutine the plugin created"
        )));
    }
    let dir = std::env::temp_dir().join(format!("dictate-{}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&dir)
        .map_err(|error| mlua::Error::runtime(format!("{name}: {error}")))?;
    let path = dir.join(format!(
        "{}.{extension}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if let Some(files) = lua.app_data_ref::<TempFiles>() {
        files
            .0
            .borrow_mut()
            .entry(current)
            .or_default()
            .push(path.clone());
    }
    Ok(path)
}

pub(crate) fn install(lua: &Lua) {
    lua.set_app_data(Tasks::default());
    lua.set_app_data(Resumed::default());
    lua.set_app_data(TempFiles::default());
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
    lua.yield_with::<()>(PendingCall(RefCell::new(Some(call))))
        .await?;
    lua.app_data_ref::<Resumed>()
        .and_then(|resumed| resumed.0.borrow_mut().remove(&current))
        .ok_or_else(|| mlua::Error::runtime(format!("{name} was resumed without a reply")))
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
}

impl Task {
    pub(crate) fn new(lua: &Lua, plugin: &str, function: &Function) -> mlua::Result<Self> {
        let thread = lua.create_thread(function.clone())?;
        if let Some(tasks) = lua.app_data_ref::<Tasks>() {
            tasks.0.borrow_mut().insert(thread.to_pointer());
        }
        Ok(Self {
            lua: lua.clone(),
            thread,
            plugin: plugin.to_owned(),
        })
    }

    pub(crate) fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Call the task's function with `args` and run until it finishes,
    /// fails, or parks on a call.
    pub(crate) fn start(&mut self, lua: &Lua, args: impl IntoLuaMulti) -> Step {
        self.step(lua, args)
    }

    /// Answer the call the task is parked on with `values` and run until it
    /// finishes, fails, or parks again.
    pub(crate) fn resume(&mut self, lua: &Lua, values: impl IntoLuaMulti) -> Step {
        let values = match values.into_lua_multi(lua) {
            Ok(values) => values,
            Err(error) => return Step::Failed(error),
        };
        if let Some(resumed) = lua.app_data_ref::<Resumed>() {
            resumed
                .0
                .borrow_mut()
                .insert(self.thread.to_pointer(), values);
        }
        self.step(lua, ())
    }

    fn step(&mut self, lua: &Lua, args: impl IntoLuaMulti) -> Step {
        set_active(lua, Some(&self.plugin));
        let result = self.thread.resume::<MultiValue>(args);
        set_active(lua, None);

        match result {
            Err(error) => Step::Failed(error),
            Ok(values) if self.thread.status() != ThreadStatus::Resumable => Step::Done(values),
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
        let mut step = self.start(lua, args);
        loop {
            match step {
                Step::Done(values) => return Ok(values),
                Step::Failed(error) => return Err(error),
                Step::Parked(Call::Exec(request)) => {
                    let outcome = crate::exec::run(&request, &AtomicBool::new(false));
                    step = self.resume(lua, crate::exec::into_lua(lua, outcome)?);
                }
                Step::Parked(Call::Core(_) | Call::Hooks { .. }) => {
                    let refused = CoreReplyValue(Err(
                        "this call only works in event handlers and hooks, not while plugins load"
                            .to_owned(),
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
        if let Some(resumed) = self.lua.app_data_ref::<Resumed>() {
            resumed.0.borrow_mut().remove(&self.thread.to_pointer());
        }
        let files = self
            .lua
            .app_data_ref::<TempFiles>()
            .and_then(|files| files.0.borrow_mut().remove(&self.thread.to_pointer()));
        for path in files.into_iter().flatten() {
            drop(std::fs::remove_file(&path));
            // Fails, harmlessly, while another call still has a file there.
            if let Some(dir) = path.parent() {
                drop(std::fs::remove_dir(dir));
            }
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
