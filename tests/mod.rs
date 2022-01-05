use assert_cmd::{crate_name, Command};

fn run() -> Command {
    let root_path = env!("CARGO_MANIFEST_DIR").to_string();
    let mut command = Command::new(root_path.clone() + "/target/debug/" + crate_name!());
    command.current_dir(root_path + "/test");
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
\trun\n",
    );
}
