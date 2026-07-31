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
    host_libc: bool,
    jni_onload: bool,
    verbose: bool,
}

const USAGE: &str = "\
usage: cordial-load --lib-dir <dir> [options]

  --lib-dir <dir>   directory holding the APK's lib/x86_64/ objects
  --library <name>  object to load (default: libroblox.so)
  --host-libc       also resolve libc from the host (ABI-unsafe; diagnostic only)
  --jni-onload      call JNI_OnLoad after loading (expect this to fail)
  -v, --verbose     list every symbol and how it resolved

env:
  MCPELAUNCHER_LINKER_VERBOSITY=<n>  bionic linker tracing (try 1 or 2)
  CORDIAL_STUB_ABORT=1               abort on the first unimplemented call
  CORDIAL_STUB_QUIET=1               do not report stub hits as they happen
  CORDIAL_TRACE=1                    log libc calls Roblox makes (loud)
";

fn parse() -> Result<Options, String> {
    let mut opt = Options {
        lib_dir: String::new(),
        library: "libroblox.so".into(),
        host_libc: false,
        jni_onload: false,
        verbose: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lib-dir" => opt.lib_dir = args.next().ok_or("--lib-dir needs a value")?,
            "--library" => opt.library = args.next().ok_or("--library needs a value")?,
            "--host-libc" => opt.host_libc = true,
            "--jni-onload" => opt.jni_onload = true,
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
            println!("\ncalling JNI_OnLoad(null, null) — expect this to fail");
            // SAFETY: nothing about this is safe. It calls into a 116 MB binary
            // whose libc is mostly stubs, with a null JavaVM. It exists to find
            // out where it stops.
            let f: extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32 =
                unsafe { std::mem::transmute(p) };
            let rc = f(std::ptr::null_mut(), std::ptr::null_mut());
            println!("JNI_OnLoad returned {rc} ({rc:#x})");
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
