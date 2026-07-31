use std::env;

struct BuildIdentity {
    display_name: &'static str,
    config_directory: &'static str,
    socket_file: &'static str,
    overlay_app_id: &'static str,
    overlay_namespace: &'static str,
    debug_app_id: &'static str,
}

const STABLE: BuildIdentity = BuildIdentity {
    display_name: "Dictate",
    config_directory: "dictate",
    socket_file: "dictate.sock",
    overlay_app_id: "dev.joshthomas.dictate.gpui",
    overlay_namespace: "dictate-overlay",
    debug_app_id: "dev.joshthomas.dictate.debug",
};

const DEV: BuildIdentity = BuildIdentity {
    display_name: "Dictate Dev",
    config_directory: "dictate-dev",
    socket_file: "dictate-dev.sock",
    overlay_app_id: "dev.joshthomas.dictate-dev.gpui",
    overlay_namespace: "dictate-dev-overlay",
    debug_app_id: "dev.joshthomas.dictate-dev.debug",
};

fn main() {
    println!("cargo::rerun-if-env-changed=DICTATE_BUILD");

    let identity = match env::var("DICTATE_BUILD") {
        Ok(value) if value == "dev" => &DEV,
        Ok(value) if value == "stable" => &STABLE,
        Err(env::VarError::NotPresent) => &STABLE,
        Ok(value) => panic!("DICTATE_BUILD must be `stable` or `dev`, got {value:?}"),
        Err(error) => panic!("could not read DICTATE_BUILD: {error}"),
    };

    emit("DICTATE_DISPLAY_NAME", identity.display_name);
    emit("DICTATE_CONFIG_DIRECTORY", identity.config_directory);
    emit("DICTATE_SOCKET_FILE", identity.socket_file);
    emit("DICTATE_OVERLAY_APP_ID", identity.overlay_app_id);
    emit("DICTATE_OVERLAY_NAMESPACE", identity.overlay_namespace);
    emit("DICTATE_DEBUG_APP_ID", identity.debug_app_id);
}

fn emit(name: &str, value: &str) {
    println!("cargo::rustc-env={name}={value}");
}
