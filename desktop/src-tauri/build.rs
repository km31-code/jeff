fn main() {
    // apex f1c: the jeff_daemon binary links the full GUI framework stack
    // transitively (tauri/wry/appkit/webkit). launched headless by launchd -- with
    // no app bundle and no window server session -- it hangs in early framework
    // init trying to connect to the window server, before it ever binds its socket.
    // embedding an Info.plist section with LSBackgroundOnly marks the process as
    // background-only so those frameworks skip the GUI handshake. this affects only
    // the daemon binary; the tauri app gets its Info.plist from bundling.
    #[cfg(target_os = "macos")]
    {
        let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("daemon_info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!(
            "cargo:rustc-link-arg-bin=jeff_daemon=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }

    tauri_build::build()
}
