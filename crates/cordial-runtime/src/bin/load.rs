//! `cordial-load` — load `libroblox.so` with the bionic linker.
//!
//! This does not run Roblox. It proves the loader, the relocations and the TLS
//! layout work against the real 116 MB object, and turns
//! docs/framework-api-inventory.md into a prioritised list of what to implement.

use std::process::ExitCode;
use std::time::Instant;

use cordial_linker_sys as linker;
use cordial_runtime::{stubs, symtab};

struct Options {
    lib_dir: String,
    library: String,
    apk: Option<String>,
    read_asset: Option<String>,
    client_settings: Option<String>,
    gl_probe: bool,
    window_seconds: Option<u64>,
    game_activity: bool,
    run_seconds: u64,
    host_libc: bool,
    jni_onload: bool,
    dump_classes: Option<String>,
    verbose: bool,
}

const USAGE: &str = "\
usage: cordial-load --lib-dir <dir> [options]

  --lib-dir <dir>   directory holding the APK's lib/x86_64/ objects
  --library <name>  object to load (default: libroblox.so)
  --apk <path>      APK to serve assets from; without it AAssetManager_open fails
  --read-asset <p>  read one asset through the AAsset API and report its size
  --client-settings <f>  Roblox ClientSettings JSON; the engine reports
                    onFlagsFailed without it
  --gl-probe        bring up GLES2 through the symbol table and read a pixel back
  --window <secs>   GL PROBE ONLY: open a window and draw a gradient for <secs>.
                    This is Cordial's own test pattern, not Roblox rendering.
  --host-libc       also resolve libc from the host (ABI-unsafe; diagnostic only)
  --jni-onload      stand up a JavaVM and call JNI_OnLoad
  --game-activity   implies --jni-onload; bring Roblox up and hand it a surface
  --run <secs>      how long to let Roblox run after handover (default 15)
  --dump-classes <f>  implies --jni-onload; write the Java classes Roblox asked
                    for to <f> — the observed Phase 2 backlog
  -v, --verbose     list every symbol and how it resolved

env:
  MCPELAUNCHER_LINKER_VERBOSITY=<n>  bionic linker tracing (try 1 or 2)
  CORDIAL_STUB_ABORT=1               abort on the first unimplemented call
  CORDIAL_STUB_QUIET=1               do not report stub hits as they happen
  CORDIAL_TRACE=1                    log libc calls (WARNING: wraps variadic
                                     functions with fixed-arity ones, which is
                                     not ABI-safe — it changes behaviour)
  CORDIAL_ANDROID_TRACE=1            log Android API calls (safe; no variadics)
";

fn parse() -> Result<Options, String> {
    let mut opt = Options {
        lib_dir: String::new(),
        library: "libroblox.so".into(),
        apk: None,
        read_asset: None,
        client_settings: None,
        gl_probe: false,
        window_seconds: None,
        game_activity: false,
        run_seconds: 15,
        host_libc: false,
        jni_onload: false,
        dump_classes: None,
        verbose: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lib-dir" => opt.lib_dir = args.next().ok_or("--lib-dir needs a value")?,
            "--library" => opt.library = args.next().ok_or("--library needs a value")?,
            "--apk" => opt.apk = Some(args.next().ok_or("--apk needs a path")?),
            "--read-asset" => {
                opt.read_asset = Some(args.next().ok_or("--read-asset needs a name")?)
            }
            "--client-settings" => {
                opt.client_settings =
                    Some(args.next().ok_or("--client-settings needs a path")?)
            }
            "--gl-probe" => opt.gl_probe = true,
            "--window" => {
                let v = args.next().ok_or("--window needs a duration in seconds")?;
                opt.window_seconds = Some(v.parse().map_err(|_| "--window wants a number")?);
            }
            "--host-libc" => opt.host_libc = true,
            "--jni-onload" => opt.jni_onload = true,
            "--run" => {
                let v = args.next().ok_or("--run needs a duration in seconds")?;
                opt.run_seconds = v.parse().map_err(|_| "--run wants a number")?;
            }
            "--game-activity" => {
                opt.jni_onload = true;
                opt.game_activity = true;
            }
            "--dump-classes" => {
                opt.jni_onload = true;
                opt.dump_classes = Some(args.next().ok_or("--dump-classes needs a path")?);
            }
            "-v" | "--verbose" => opt.verbose = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    if opt.lib_dir.is_empty() {
        return Err("--lib-dir is required".into());
    }
    Ok(opt)
}

fn main() -> ExitCode {
    let opt = match parse() {
        Ok(o) => o,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    if let Some(apk) = &opt.apk {
        match cordial_runtime::android::asset::set_apk(std::path::Path::new(apk)) {
            Ok(()) => println!("assets: {apk}"),
            Err(e) => {
                eprintln!("bad --apk: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if let Some(name) = &opt.read_asset {
        match cordial_runtime::android::asset::probe(name) {
            Ok(len) => println!("asset {name}: {len} bytes"),
            Err(e) => {
                eprintln!("asset {name}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    cordial_runtime::android::set_trace(std::env::var_os("CORDIAL_ANDROID_TRACE").is_some());

    let table = symtab::build(opt.host_libc);
    let totals = table.totals();

    println!(
        "symbol table: {} cordial, {} host, {} stub, {} total across {} libraries",
        totals.cordial,
        totals.host,
        totals.stub,
        totals.cordial + totals.host + totals.stub,
        table.libraries.len()
    );
    for (lib, s) in &table.stats {
        println!(
            "  {lib:<20} cordial={:<4} host={:<5} stub={}",
            s.cordial, s.host, s.stub
        );
    }
    for missing in &table.missing_host_libs {
        println!("  warning: host {missing} unavailable; its symbols are stubbed");
    }

    if opt.verbose {
        for (lib, entries) in &table.libraries {
            for e in entries {
                println!(
                    "  {lib:<20} {:<44} {}",
                    e.symbol,
                    e.source.label()
                );
            }
        }
    }

    if let Some(secs) = opt.window_seconds {
        match cordial_runtime::android::gl::probe_window(&table, secs) {
            Ok(r) => {
                println!("\nGL probe rendered into a real window (this is a test pattern, not Roblox):");
                println!("  renderer  {}", r.renderer);
                println!("  version   {}", r.version);
                println!("  readback  {:02x?}", r.pixel);
            }
            Err(e) => {
                eprintln!("\nwindow render failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if opt.gl_probe {
        match cordial_runtime::android::gl::probe(&table) {
            Ok(r) => {
                println!("\nGLES2 context is live:");
                println!("  vendor    {}", r.vendor);
                println!("  renderer  {}", r.renderer);
                println!("  version   {}", r.version);
                println!("  readback  {:02x?} — drew and read it back", r.pixel);
            }
            Err(e) => {
                eprintln!("\nGL probe failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\ninitialising bionic linker...");
    linker::init();

    for (name, entries) in &table.libraries {
        let symbols: Vec<(String, *mut std::ffi::c_void)> = entries
            .iter()
            .map(|e| (e.symbol.to_string(), e.address))
            .collect();
        if let Err(e) = linker::register(name, &symbols) {
            eprintln!("failed to register {name}: {e}");
            return ExitCode::FAILURE;
        }
    }
    println!("registered {} virtual libraries", table.libraries.len());

    if let Err(e) = linker::set_library_path(&opt.lib_dir) {
        eprintln!("bad --lib-dir: {e}");
        return ExitCode::FAILURE;
    }
    println!("search path: {}", opt.lib_dir);

    println!("\nloading {} ...", opt.library);

    let start = Instant::now();
    let result = linker::dlopen(&opt.library, linker::RTLD_NOW);
    let elapsed = start.elapsed();

    let lib = match result {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("\nLOAD FAILED after {:.0?}: {e}", elapsed);
            stubs::report();
            return ExitCode::FAILURE;
        }
    };

    let (code_base, code_size) = lib.code_region();
    println!("\nLOADED in {:.0?}", elapsed);
    println!("  base       {:#x}", lib.base());
    println!(
        "  code       {code_base:#x} + {code_size} bytes ({:.1} MB)",
        code_size as f64 / (1024.0 * 1024.0)
    );

    match lib.symbol("JNI_OnLoad") {
        Some(p) => println!("  JNI_OnLoad {p:p}"),
        None => println!("  JNI_OnLoad not found"),
    }

    if opt.jni_onload {
        if let Some(p) = lib.symbol("JNI_OnLoad") {
            let Some(vm) = linker::jni::create_vm() else {
                eprintln!("could not create a JavaVM");
                return ExitCode::FAILURE;
            };
            println!("\nJavaVM at {vm:p}; calling JNI_OnLoad");

            match linker::jni::call_on_load(p) {
                // JNI versions are 0x000M_000m; 0x00010006 is JNI_VERSION_1_6.
                Ok(rc) => {
                    println!("JNI_OnLoad returned {rc:#x} = JNI {}.{}", rc >> 16, rc & 0xffff);
                }
                Err(e) => {
                    println!("JNI_OnLoad failed: {e}");
                    println!(
                        "\n  Roblox expects the Android bring-up sequence, not a bare JNI_OnLoad:\n                           a JavaVM, then GameActivity.initializeNativeCode called from Java with a\n                           real Activity. See docs/framework-api-inventory.md §3.3."
                    );
                }
            }

            if opt.game_activity {
                let native = lib.symbol(
                    "Java_com_google_androidgamesdk_GameActivity_initializeNativeCode",
                );
                match native {
                    None => eprintln!("  initializeNativeCode is not exported"),
                    Some(f) => {
                        let files = std::env::var("CORDIAL_FILES_DIR").unwrap_or_else(|_| {
                            format!(
                                "{}/cordial/instances/default/data",
                                std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!(
                                    "{}/.local/share",
                                    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                                ))
                            )
                        });
                        // Android's framework prepares the UI thread's looper
                        // before any app code runs, and AGDK's
                        // initializeNativeCode bails out with a zero handle if
                        // ALooper_forThread returns null. Nothing else prepares
                        // one here.
                        if !cordial_runtime::android::looper::prepare_for_current_thread() {
                            eprintln!("  could not prepare a looper for this thread");
                            return ExitCode::FAILURE;
                        }

                        println!("\ncalling GameActivity.initializeNativeCode");
                        match linker::game_activity::initialize(f, &files, &files, &files) {
                            Ok(handle) => {
                                println!("  native handle {handle:#x}");

                                // The engine renders into an ANativeWindow, so
                                // there has to be a real one before the surface
                                // callbacks arrive.
                                match cordial_runtime::android::window::open(
                                    1280, 720, &cordial_runtime::window_title("OpenGL ES"),
                                ) {
                                    Err(e) => println!("  no window: {e}"),
                                    Ok(w) => {
                                        let (width, height, format) = w.geometry();
                                        cordial_runtime::android::config::set_screen(width, height);
                                        println!("  window {width}x{height}");
                                        cordial_runtime::android::config::set_screen(width, height);

                                        // The engine's own init sequence, in the
                                        // order MainGameActivity.onCreate runs it.
                                        // Without the asset manager the engine
                                        // cannot read its own content, which is
                                        // why nothing downstream ever starts —
                                        // no app shell, no network, no frame.
                                        let files = files.clone();
                                        let steps: Vec<(&str, Box<dyn Fn(*mut std::ffi::c_void) -> Result<(), String>>)> = vec![
                                            (
                                                "Java_com_roblox_client_JNIAAssetManagerSetup_initNative",
                                                Box::new(linker::game_activity::asset_manager_init),
                                            ),
                                            (
                                                "Java_com_roblox_client_LocalStorageManager_initStorageManagerNativeV3",
                                                Box::new(move |f| {
                                                    linker::game_activity::storage_init(f, &files, &files)
                                                }),
                                            ),
                                        ];
                                        for (name, run) in steps {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match run(f) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }
                                        if let Some(p) = lib.symbol(
                                            "Java_com_roblox_client_startup_MainGameActivity_nativeAppBridgeSetInitParams",
                                        ) {
                                            match linker::game_activity::set_init_params(
                                                p,
                                                opt.apk.as_deref().unwrap_or(""),
                                                width,
                                                height,
                                            ) {
                                                Ok(()) => println!("  init params set"),
                                                Err(e) => println!("  init params failed: {e}"),
                                            }
                                        }

                                        // Flags before anything else asks for
                                        // them: bootstrapTheApp's whole job is to
                                        // reach this, and the engine reports
                                        // onFlagsFailed without it.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_client_flags_FlagJniInterface_nativeInitializeNativeFlags",
                                        ) {
                                            let settings = opt
                                                .client_settings
                                                .as_deref()
                                                .and_then(|p| std::fs::read_to_string(p).ok())
                                                .unwrap_or_default();
                                            match linker::game_activity::init_flags(f, &settings) {
                                                Ok(()) => println!("  flags initialised"),
                                                Err(e) => println!("  flag init failed: {e}"),
                                            }
                                        }

                                        // Kicks the engine's initialisation once
                                        // everything it depends on is in place.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_client_startup_MainGameActivity_nativeRetryInit",
                                        ) {
                                            match linker::game_activity::call_bare(f) {
                                                Ok(()) => println!("  retryInit ok"),
                                                Err(e) => println!("  retryInit failed: {e}"),
                                            }
                                        }

                                        // Globals first. Disassembly of the
                                        // ActivityNativeMain chain gives this
                                        // order, and calling StartLuaAppDM
                                        // without them crashes on a null JNIEnv
                                        // the engine expects to have been stored
                                        // by the globals init.
                                        for name in [
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeGameGlobalInit",
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeUpdateAdapterInit",
                                        ] {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match linker::game_activity::appbridge_call_bare(f) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }

                                        // The app bridge proper. ActivitySplash —
                                        // the only launcher Activity — defaults
                                        // to ActivityNativeMain, not the AGDK
                                        // MainGameActivity, and this is the chain
                                        // that actually brings the client up.
                                        let apk_path = opt.apk.clone().unwrap_or_default();
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2InitWithParams",
                                        ) {
                                            match linker::game_activity::appbridge_init(
                                                f, &apk_path, width, height,
                                            ) {
                                                Ok(()) => println!("  app bridge initialised"),
                                                Err(e) => println!("  app bridge init failed: {e}"),
                                            }
                                        }
                                        if std::env::var_os("CORDIAL_SKIP_LUA_DM").is_none() {
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeStartLuaAppDM",
                                        ) {
                                            match linker::game_activity::appbridge_call_bare(f) {
                                                Ok(()) => println!("  Lua app DataModel started"),
                                                Err(e) => println!("  StartLuaAppDM failed: {e}"),
                                            }
                                        }
                                        }

                                        // And the call that delivers the surface.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2StartAppWithParams",
                                        ) {
                                            match linker::game_activity::appbridge_start_app(
                                                f, &apk_path, width, height,
                                            ) {
                                                Ok(()) => println!("  app started with surface"),
                                                Err(e) => println!("  StartApp failed: {e}"),
                                            }
                                        }

                                        match linker::game_activity::start(
                                            handle, width, height, format,
                                        ) {
                                            Ok(()) => {
                                                println!("  surface handed to the engine");
                                                let secs = opt.run_seconds;
                                                println!("  pumping the looper for {secs}s");
                                                // Android's UI thread runs the
                                                // message loop; AGDK put its
                                                // pipes on this thread's looper.
                                                cordial_runtime::android::looper::pump(
                                                    std::time::Duration::from_secs(secs),
                                                );
                                                if std::env::var_os("CORDIAL_COUNT_GL").is_some() {
                                                    // What each thread is blocked on. A game thread
                                                    // waiting on a socket, a futex, or nothing at all
                                                    // are three different problems.
                                                    println!("\n  threads:");
                                                    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
                                                        for e in dir.flatten() {
                                                            let p = e.path();
                                                            let name = std::fs::read_to_string(p.join("comm"))
                                                                .unwrap_or_default().trim().to_string();
                                                            let wchan = std::fs::read_to_string(p.join("wchan"))
                                                                .unwrap_or_default().trim().to_string();
                                                            let state = std::fs::read_to_string(p.join("stat"))
                                                                .ok()
                                                                .and_then(|s| s.rsplit(')').next()
                                                                    .and_then(|r| r.split_whitespace().next())
                                                                    .map(str::to_string))
                                                                .unwrap_or_default();
                                                            println!("    {name:<18} state={state:<2} wchan={wchan}");
                                                        }
                                                    }
                                                    println!(
                                                        "  looper polls: {}",
                                                        cordial_runtime::android::looper::POLLS
                                                            .load(std::sync::atomic::Ordering::Relaxed)
                                                    );
                                                    println!("\n  graphics calls Roblox made:");
                                                    for (name, n) in
                                                        cordial_runtime::android::glcount::report()
                                                    {
                                                        println!("    {name:<24} {n}");
                                                    }
                                                }
                                            }
                                            Err(e) => println!("  lifecycle failed: {e}"),
                                        }
                                    }
                                }
                            }
                            Err(e) => println!("  failed: {e}"),
                        }
                    }
                }
            }

            if let Some(path) = &opt.dump_classes {
                match linker::jni::dump_classes(path) {
                    Ok(()) => println!("  Java classes Roblox reached for -> {path}"),
                    Err(e) => eprintln!("  class dump failed: {e}"),
                }
            }
        }
    }

    stubs::report();

    // Leave via _exit rather than returning.
    //
    // Roblox's static initialisers registered atexit handlers and DT_FINI_ARRAY
    // destructors that expect a live Android process — a JavaVM, a working
    // looper, its own stdio. Running them here segfaults during teardown, long
    // after the load this tool exists to verify has already succeeded. Clean
    // shutdown belongs with instance lifecycle in core; until then, reporting a
    // teardown crash as a load failure would be actively misleading.
    //
    // SAFETY: _exit is async-signal-safe and terminates without running any
    // handler. Nothing here owns a resource the kernel will not reclaim.
    unsafe { libc_exit(0) }
}

extern "C" {
    #[link_name = "_exit"]
    fn libc_exit(status: std::ffi::c_int) -> !;
}
