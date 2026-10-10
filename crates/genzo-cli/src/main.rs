//! 検証用の CLI（`genzo`）。処理は lib（`genzo_cli`）にある（ORG-05。04 の 1.4 節）。

fn main() -> std::process::ExitCode {
    genzo_cli::main_entry()
}
