use std::env;
use std::ffi::{OsStr, OsString};
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

    eprintln!("unknown temper-dev command: {}", command.to_string_lossy());
    print_usage(&program);
    process::exit(2);
}

fn print_usage(program: &OsStr) {
    eprintln!(
        "usage: {} dev-test-full [nextest-args...]",
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

fn has_passthrough_arg(args: &[OsString], needle: &str) -> bool {
    args.iter().any(|arg| arg == OsStr::new(needle))
}

fn run_cargo(fixed_args: &[&OsStr], passthrough_args: &[OsString]) -> i32 {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    eprintln!(
        "temper-dev: running cargo {}{}",
        display_args(fixed_args.iter().copied()),
        display_args(passthrough_args.iter().map(OsString::as_os_str))
    );

    let status = Command::new(&cargo)
        .args(fixed_args)
        .args(passthrough_args)
        .status();

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
    args.map(|arg| format!(" {}", arg.to_string_lossy()))
        .collect()
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
