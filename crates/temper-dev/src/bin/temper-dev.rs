use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{self, Command, ExitStatus};

fn main() {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("temper-dev"));

    let Some(command) = args.next() else {
        print_usage(&program);
        process::exit(2);
    };

    if command == OsStr::new("dev-test-full") {
        let passthrough_args: Vec<OsString> = args.collect();
        process::exit(run_dev_test_full(&passthrough_args));
    }
    if command == OsStr::new("dev-scenario-run-live") {
        let scenario_args: Vec<OsString> = args.collect();
        process::exit(run_dev_scenario_run_live(&scenario_args));
    }

    eprintln!("unknown temper-dev command: {}", command.to_string_lossy());
    print_usage(&program);
    process::exit(2);
}

fn print_usage(program: &OsStr) {
    eprintln!(
        "usage: {} <command>\n\ncommands:\n  dev-test-full [nextest-args...]\n  dev-scenario-run-live [scenario-path]",
        program.to_string_lossy()
    );
}

fn run_dev_test_full(passthrough_args: &[OsString]) -> i32 {
    let quick_status = if has_passthrough_arg(passthrough_args, "--no-run") {
        run_cargo(&[OsStr::new("dev-test-quick")], passthrough_args)
    } else {
        run_cargo(
            &[OsStr::new("dev-test-quick"), OsStr::new("--no-fail-fast")],
            passthrough_args,
        )
    };

    let capstone_status = run_cargo(&[OsStr::new("dev-test-e2e-capstones")], passthrough_args);

    if quick_status == 0 {
        capstone_status
    } else {
        quick_status
    }
}

fn run_dev_scenario_run_live(args: &[OsString]) -> i32 {
    let scenario = match args {
        [] => OsString::from("scenarios/basic-delivery"),
        [scenario] => scenario.clone(),
        [_, extra, ..] => {
            eprintln!(
                "temper-dev: unexpected dev-scenario-run-live argument: {}",
                extra.to_string_lossy()
            );
            return 2;
        }
    };

    let build_status = run_cargo(
        &[
            OsStr::new("build"),
            OsStr::new("--bin"),
            OsStr::new("temper"),
        ],
        &[],
    );
    if build_status != 0 {
        return build_status;
    }

    run_cargo_owned(&[
        OsString::from("run"),
        OsString::from("-p"),
        OsString::from("temper-scenario-cli"),
        OsString::from("--"),
        OsString::from("run"),
        OsString::from("--tier"),
        OsString::from("live"),
        OsString::from("--temper-bin"),
        target_debug_temper_binary().into_os_string(),
        scenario,
    ])
}

fn target_debug_temper_binary() -> PathBuf {
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    target_dir
        .join("debug")
        .join(format!("temper{}", env::consts::EXE_SUFFIX))
}

fn has_passthrough_arg(args: &[OsString], needle: &str) -> bool {
    args.iter().any(|arg| arg == OsStr::new(needle))
}

fn run_cargo(fixed_args: &[&OsStr], passthrough_args: &[OsString]) -> i32 {
    let args = fixed_args
        .iter()
        .map(|arg| (*arg).to_os_string())
        .chain(passthrough_args.iter().cloned())
        .collect::<Vec<_>>();
    run_cargo_owned(&args)
}

fn run_cargo_owned(args: &[OsString]) -> i32 {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    eprintln!(
        "temper-dev: running cargo{}",
        display_args(args.iter().map(OsString::as_os_str))
    );

    let status = Command::new(&cargo).args(args).status();

    match status {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!(
                "temper-dev: failed to run {}: {error}",
                cargo.to_string_lossy()
            );
            1
        }
    }
}

fn display_args<'a>(args: impl Iterator<Item = &'a OsStr>) -> String {
    let mut display = String::new();
    for arg in args {
        let _ = write!(&mut display, " {}", arg.to_string_lossy());
    }
    display
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
