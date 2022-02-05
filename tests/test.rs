use assert_cmd::Command;

fn run() -> Command {
    let mut command = Command::cargo_bin(env!("CARGO_PKG_NAME")).unwrap();
    command.current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/test_data");
    command
}

#[test]
fn list_tasks() {
    run().assert().success().stderr("").stdout(
        "Available tasks:
\tbuild - build stuff
\tserver
\tclient - server
\tstraw-task
\trun
\traw
\traw-explicit
\tnot-raw-explicit\n",
    );
}

#[test]
fn run_task() {
    run().arg("build").assert().success().stderr("").stdout(
        "> build
\x1b[0;32mbuild:\x1b[0m build
finished build\n",
    );
}

const NON_EXISTENT_TASK: &str = "this-task-is-not-in-the-pilotfile";

#[test]
fn run_non_existent_task() {
    run()
        .arg(NON_EXISTENT_TASK)
        .assert()
        .failure()
        .stdout("> ".to_string() + NON_EXISTENT_TASK + "\n")
        .stderr("Task ".to_string() + NON_EXISTENT_TASK + " not found in Pilotfile\n");
}

#[test]
fn run_multiple_tasks() {
    run()
        .arg("client")
        .arg("build")
        .assert()
        .success()
        .stderr("")
        .stdout(
            "> client
\x1b[0;32mclient:\x1b[0m client
finished client
> build
\x1b[0;32mbuild:\x1b[0m  build
finished build\n",
        );
}

#[test]
fn run_nested_task() {
    run()
        .arg("straw-task")
        .assert()
        .success()
        .stderr("")
        .stdout(
            "> straw-task
> straw-task > build
\x1b[0;32mbuild:\x1b[0m build
finished straw-task > build
finished straw-task\n",
        );
}

#[test]
fn run_parallel_tasks() {
    run().arg("run").assert().success().stderr("").stdout(
        "> run
> run > build
\x1b[0;32mbuild:\x1b[0m build
finished run > build
> run > server
> run > client
> run > straw-task
> run > straw-task > build
\x1b[0;33mclient:\x1b[0m client
finished run > client
\x1b[0;34mbuild:\x1b[0m  build
finished run > straw-task > build
finished run > straw-task
\x1b[0;32mserver:\x1b[0m server
finished run > server
finished run\n",
    );
}

const TEST_INPUT: &str = "test-input";

#[test]
fn run_raw() {
    run()
        .arg("raw")
        .arg("-r")
        .write_stdin(TEST_INPUT.to_string() + "\n")
        .assert()
        .success()
        .stderr("")
        .stdout("> raw\n".to_string() + TEST_INPUT + "\nfinished raw\n");
}

#[test]
fn run_tasks_explicit_raw_not_raw() {
    run()
        .arg("raw-explicit")
        .write_stdin(TEST_INPUT.to_string() + "\n")
        .assert()
        .success()
        .stderr("")
        .stdout(
            "> raw-explicit\n".to_string()
                + TEST_INPUT
                + "
> raw-explicit > not-raw-explicit
\x1b[0;32mnot-raw-explicit:\x1b[0m not raw
finished raw-explicit > not-raw-explicit
finished raw-explicit\n",
        );
}

#[test]
fn find_pilotfile_in_parent_dir() {
    let mut command = Command::cargo_bin(env!("CARGO_PKG_NAME")).unwrap();
    command.current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/test_data/sub_dir");

    // basically same as list_tasks
    command.assert().success().stderr("").stdout(
        "Available tasks:
\tbuild - build stuff
\tserver
\tclient - server
\tstraw-task
\trun
\traw
\traw-explicit
\tnot-raw-explicit\n",
    );
}
