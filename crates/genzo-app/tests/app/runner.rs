//! 結合テストの小さな実行器（libtest の代わり。`harness = false` のため）。
//!
//! libtest の引数のうち、よく使うものだけを受け付ける。
//!
//! | 引数 | 意味 |
//! |---|---|
//! | 名前の一部（複数可） | その文字列を含むテストだけを実行する |
//! | `--exact` | 名前が完全に一致するものだけ |
//! | `--skip <文字列>` | その文字列を含むテストを除く |
//! | `--list` | テストの名前を `名前: test` の形で一覧にする（IDE・テストの一覧の取得用） |
//! | `--ignored` | 無視するテストだけ（このテストにはないので、何も実行しない） |
//! | `--nocapture`・`--show-output`・`-q`・`--quiet`・`--include-ignored`・`--test-threads <n>`・`--color <c>`・`--format <f>` | 受け付けて無視する（出力は取り込まず、テストは常に 1 つずつ実行する） |
//!
//! # Tauri の CLI が `[[bin]]` を配布物に入れることの確認
//!
//! この crate にテスト用の実行ファイル（`[[bin]]`）を足さない理由（main.rs の doc）は、次で確かめた
//! （2026-10-12。`@tauri-apps/cli` 2.11.5、Linux）。`tauri.conf.json` と Cargo.toml だけの小さなアプリに
//! `[[bin]]` を 2 つ（`mainBinaryName` のものと、もう 1 つ）と `src/bin/` のファイルを 1 つ置き、実行ファイルを
//! 用意して `tauri bundle --bundles deb` を実行すると、`.deb` の `usr/bin/` に 3 つとも入った。
//! nsis・app・dmg も同じ一覧の実行ファイルを入れると考えられるが、この環境（Linux）では作れないため
//! **要確認**。
//!
//! 失敗したテストがあれば、終了コード 101（libtest と同じ）で終わる。

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Instant;

/// 実行の条件。
#[derive(Default)]
struct Options {
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
    list: bool,
    ignored_only: bool,
}

impl Options {
    fn parse(args: &[OsString]) -> Self {
        let mut o = Self::default();
        let mut it = args.iter().map(|a| a.to_string_lossy().into_owned());
        while let Some(arg) = it.next() {
            // `--name=value` の形も受け付ける。
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
                _ => (arg.clone(), None),
            };
            let mut value = |inline: Option<String>| inline.or_else(|| it.next());
            match name.as_str() {
                "--exact" => o.exact = true,
                "--list" => o.list = true,
                "--ignored" => o.ignored_only = true,
                "--skip" => o.skips.extend(value(inline)),
                // 値を取る引数（無視する）。
                "--test-threads" | "--color" | "--format" | "--logfile" | "-Z" => {
                    let _ = value(inline);
                }
                _ if name.starts_with('-') => {}
                _ => o.filters.push(arg),
            }
        }
        o
    }

    fn selects(&self, name: &str) -> bool {
        if self.ignored_only {
            return false;
        }
        let matches = |f: &String| {
            if self.exact {
                name == f
            } else {
                name.contains(f.as_str())
            }
        };
        (self.filters.is_empty() || self.filters.iter().any(matches))
            && !self.skips.iter().any(|s| name.contains(s.as_str()))
    }
}

/// テストを実行する。
pub fn run(tests: &[(&str, fn())], args: &[OsString]) -> ExitCode {
    let options = Options::parse(args);
    let selected: Vec<&(&str, fn())> = tests
        .iter()
        .filter(|(name, _)| options.selects(name))
        .collect();
    if options.list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let filtered_out = tests.len() - selected.len();
    println!("\nrunning {} tests", selected.len());
    let started = Instant::now();
    let mut failed: Vec<&str> = Vec::new();
    for (name, test) in selected.iter().copied() {
        let t = Instant::now();
        // 失敗の内容（パニックのメッセージ）は、既定のパニックの処理が標準エラーに出す。
        let outcome = std::panic::catch_unwind(*test);
        let secs = t.elapsed().as_secs_f64();
        match outcome {
            Ok(()) => println!("test {name} ... ok ({secs:.1} 秒)"),
            Err(_) => {
                println!("test {name} ... FAILED ({secs:.1} 秒)");
                failed.push(name);
            }
        }
    }
    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; 0 ignored; 0 measured; {} filtered out; finished in {:.2}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        filtered_out,
        started.elapsed().as_secs_f64()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}

/// 実行器の引数の解釈（この実行器のテストの一覧に入れて確かめる）。
pub fn parses_libtest_arguments() {
    let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    let o = Options::parse(&args(&[
        "catalog",
        "--test-threads",
        "4",
        "--nocapture",
        "--skip=conflict",
        "--color",
        "never",
    ]));
    assert_eq!(
        o.filters,
        vec!["catalog".to_owned()],
        "値を取る引数の値は絞り込みにしない"
    );
    assert!(o.selects("catalog::open"));
    assert!(!o.selects("catalog::conflict"), "--skip");
    assert!(!o.selects("library::import"));
    let exact = Options::parse(&args(&["--exact", "a::b"]));
    assert!(exact.selects("a::b"));
    assert!(!exact.selects("a::bc"));
    assert!(Options::parse(&[]).selects("anything"));
    assert!(!Options::parse(&args(&["--ignored"])).selects("anything"));
    let list = Options::parse(&args(&["--list", "--format", "terse"]));
    assert!(list.list && list.filters.is_empty());
}
