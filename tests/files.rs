use std::path::PathBuf;

use egglog::*;
use libtest_mimic::Trial;

#[derive(Clone)]
struct Run {
    path: PathBuf,
    test_proofs: bool,
    should_fail: bool,
    resugar: bool,
}

impl Run {
    fn run(&self) {
        let _ = env_logger::builder().is_test(true).try_init();
        let program_read = std::fs::read_to_string(self.path.clone()).unwrap();
        let already_enables = program_read.starts_with("(set-option enable_proofs 1)");
        let program = if self.test_proofs && !already_enables {
            format!("(set-option enable_proofs 1)\n{}", program_read)
        } else {
            program_read
        };

        if !self.resugar {
            self.test_program(&program, "Top level error");
        } else if self.resugar {
            let mut egraph = EGraph::default();
            egraph.set_underscores_for_desugaring(3);
            let parsed = egraph.parse_program(&program).unwrap();
            // TODO can we test after term encoding instead?
            // last time I tried it spun out becuase
            // it adds term encoding to term encoding
            let desugared_str = egraph
                .process_commands(parsed, CompilerPassStop::TypecheckDesugared)
                .unwrap()
                .into_iter()
                .map(|x| x.resugar().to_string())
                .collect::<Vec<String>>()
                .join("\n");

            self.test_program(
                &desugared_str,
                &format!(
                    "Program:\n{}\n ERROR after parse, to_string, and parse again.",
                    desugared_str
                ),
            );
        }
    }

    fn test_program(&self, program: &str, message: &str) {
        let mut egraph = EGraph::default();
        if self.test_proofs {
            egraph.test_proofs = true;
        }
        egraph.set_underscores_for_desugaring(5);
        match egraph.parse_and_run_program(program) {
            Ok(msgs) => {
                if self.should_fail {
                    panic!(
                        "Program should have failed! Instead, logged:\n {}",
                        msgs.join("\n")
                    );
                } else {
                    for msg in msgs {
                        log::info!("  {}", msg);
                    }
                }
            }
            Err(err) => {
                if !self.should_fail {
                    panic!("{}: {err}", message)
                }
            }
        }
    }
}

fn generate_tests(glob: &str) -> Vec<Trial> {
    let mut trials = vec![];
    let mut mk_trial = |name: String, run: Run| {
        trials.push(Trial::test(name, move || {
            run.run();
            Ok(())
        }))
    };

    for entry in glob::glob(glob).unwrap() {
        let f = entry.unwrap();
        let name = f
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .replace(['.', '-', ' '], "_");

        let should_fail = f.to_string_lossy().contains("fail-typecheck");

        mk_trial(
            name.clone(),
            Run {
                path: f.clone(),
                should_fail,
                test_proofs: false,
                resugar: false,
            },
        );
        if !should_fail {
            mk_trial(
                format!("{name}_resugared"),
                Run {
                    path: f.clone(),
                    should_fail,
                    test_proofs: false,
                    resugar: true,
                },
            );
        }

        // make a test with proofs enabled
        // TODO: re-enable herbie, unsound, and eqsolve when proof extraction is faster
        let banned = [
            "herbie",
            "repro_unsound",
            "eqsolve",
            "before_proofs",
            "lambda",
        ];
        if !banned.contains(&name.as_str()) {
            mk_trial(
                format!("{name}_with_proofs"),
                Run {
                    path: f.clone(),
                    should_fail,
                    test_proofs: true,
                    resugar: false,
                },
            );

            if !should_fail {
                mk_trial(
                    format!("{name}_with_proofs_resugared"),
                    Run {
                        path: f.clone(),
                        should_fail,
                        test_proofs: true,
                        resugar: true,
                    },
                );
            }
        }
    }

    trials
}

fn main() {
    let args = libtest_mimic::Arguments::from_args();
    let tests = generate_tests("tests/**/*.egg");
    libtest_mimic::run(&args, tests).exit();
}
