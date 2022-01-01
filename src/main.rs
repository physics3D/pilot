use std::{
    env::args,
    error::Error,
    fs::read_to_string,
    io::{BufRead, BufReader},
    process::{exit, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    thread,
};

use yaml_rust::{Yaml, YamlLoader};

const NOT_VALID: &str = "This is not a valid Pilotfile";
const HELP_TEXT: &str = "pilot - a simple task runner/process manager

USAGE:
    pilot [OPTIONS] [tasks]

FLAGS:
    -h, --help                  print this help text

OPTIONS:
    -q, --quiet <quiet-tasks>   run the following tasks without output (to run them, you still have to add them explicitly)
    -r, --raw                   just run the tasks, without any additional output processing (useful for interactive applications)

ARGS:
    [tasks]                     the tasks you want to run

    Without any arguments pilot will print a list of all available tasks";

trait OrMsg<T> {
    fn or_msg(self, msg: &str) -> T;
}

impl<T, E: Error> OrMsg<T> for Result<T, E> {
    fn or_msg(self, msg: &str) -> T {
        match self {
            Ok(value) => value,
            Err(err) => {
                eprintln!("{}: {}", msg, err);
                exit(1);
            }
        }
    }
}

impl<T> OrMsg<T> for Option<T> {
    fn or_msg(self, msg: &str) -> T {
        match self {
            Some(value) => value,
            None => {
                eprintln!("{}", msg);
                exit(1)
            }
        }
    }
}

static INDEX: AtomicU32 = AtomicU32::new(1);

fn run_shell(command: String, task_name: String, quiet_tasks: Vec<String>, raw: bool) {
    // cycle through shell colors
    // credit: https://github.com/chrismytton/shoreman/
    let current_index = INDEX.fetch_add(1, Ordering::SeqCst);
    let color = "\x1b[0;".to_string() + &(31 + current_index % 7).to_string() + "m";

    let task_name_clone = task_name.clone();
    let color_clone = color.clone();

    if raw {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .spawn()
            .or_msg(&format!("Failed to run task {}", task_name))
            .wait()
            .or_msg(&format!("Task {} failed", task_name));
    } else {
        let mut process = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .or_msg(&format!("Failed to run task {}", task_name));

        // print all output to shell

        // stdout
        thread::spawn(move || {
            if !quiet_tasks.contains(&task_name) {
                BufReader::new(process.stdout.or_msg("Could not get stdout"))
                    .lines()
                    .filter_map(|line| line.ok())
                    .for_each(|line| println!("{}{}:\x1b[0m {}", color, task_name, line));
            }
        });

        // stderr
        BufReader::new(process.stderr.take().or_msg("Could not get stderr"))
            .lines()
            .filter_map(|line| line.ok())
            .for_each(|line| println!("{}{}:\x1b[0m {}", color_clone, task_name_clone, line));
    }

    // the process exited
    println!("{}{}\x1b[0m finished", color_clone, task_name_clone);

    // subtract one from the index
    INDEX.fetch_sub(1, Ordering::SeqCst);
}

fn run_task(
    task: (Yaml, Yaml),
    all_tasks: Yaml,
    task_prefix: String,
    task_name: String,
    quiet_tasks: Vec<String>,
    raw: bool,
) {
    match task.0.as_str().or_msg(NOT_VALID) {
        "shell" => run_shell(
            task.1.as_str().or_msg(NOT_VALID).to_string(),
            task_name,
            quiet_tasks,
            raw,
        ),
        "task" => {
            let sub_task = task.1.as_str().or_msg(NOT_VALID).to_string();
            cli_run_task(
                all_tasks,
                sub_task.clone(),
                task_prefix + " > " + &sub_task,
                quiet_tasks,
                raw,
            );
        }
        "parallel" => {
            let mut threads = vec![];

            for sub_task in task.1.as_vec().or_msg(NOT_VALID) {
                let sub_task_tuple = sub_task
                    .as_hash()
                    .or_msg(NOT_VALID)
                    .iter()
                    .nth(0)
                    .or_msg(NOT_VALID);
                let sub_task_tuple = (sub_task_tuple.0.clone(), sub_task_tuple.1.clone());
                let all_tasks_clone = all_tasks.clone();
                let task_prefix_clone = task_prefix.clone();
                let task_name_clone = task_name.clone();
                let quiet_tasks_clone = quiet_tasks.clone();

                threads.push(thread::spawn(move || {
                    run_task(
                        sub_task_tuple,
                        all_tasks_clone,
                        task_prefix_clone,
                        task_name_clone,
                        quiet_tasks_clone,
                        raw,
                    );
                }));
            }

            for thread in threads {
                thread.join().unwrap();
            }
        }
        "description" => {}
        _ => {
            eprintln!("Unkown token");
            exit(1);
        }
    }
}

fn cli_run_task(
    yaml: Yaml,
    task: String,
    task_prefix: String,
    quiet_tasks: Vec<String>,
    raw: bool,
) {
    println!("> {}", task_prefix);

    let found_tasks: Vec<_> = yaml
        .as_hash()
        .or_msg(NOT_VALID)
        .iter()
        .filter(|yaml| yaml.0.as_str().unwrap_or("") == task)
        .collect();

    match found_tasks.len() {
        0 => {
            eprintln!("Task {} not found in Pilotfile", task);
            exit(1);
        }
        1 => {
            for sub_task in found_tasks[0].1.as_vec().or_msg(NOT_VALID) {
                let list: Vec<_> = sub_task
                    .as_hash()
                    .or_msg(NOT_VALID)
                    .iter()
                    .map(|(first, second)| (first.clone(), second.clone()))
                    .collect();
                run_task(
                    list[0].clone(),
                    yaml.clone(),
                    task_prefix.clone(),
                    task.clone(),
                    quiet_tasks.clone(),
                    raw,
                );
            }
        }
        _ => {
            eprintln!("Duplicate task {}", task);
            exit(1);
        }
    }
}

fn task_to_string(task: (&Yaml, &Yaml)) -> String {
    let task_name = task.0.as_str().or_msg(NOT_VALID);

    let vec = task.1.as_vec().or_msg(NOT_VALID);

    let descriptions: Vec<_> = vec
        .iter()
        .map(|yaml| yaml.as_hash().or_msg(NOT_VALID))
        .filter(|list| list.contains_key(&Yaml::String("description".to_string())))
        .collect();

    match descriptions.len() {
        0 => format!("{}", task_name),
        1 => match &descriptions[0][&Yaml::String("description".to_string())] {
            Yaml::String(description) => format!("{} - {}", task_name, description),
            _ => format!("{}", task_name),
        },
        _ => {
            eprintln!("More than one description for task {}", task_name);
            exit(1);
        }
    }
}

fn cli_list_tasks(yaml: &Yaml) {
    println!("Avalaible tasks:");

    for task in yaml.as_hash().or_msg(NOT_VALID) {
        println!("\t{}", task_to_string(task));
    }
}

fn load_pilotfile() -> Yaml {
    let file = read_to_string("Pilotfile.yaml").or_msg("Pilotfile.yaml not found");
    let vec = YamlLoader::load_from_str(&file).or_msg("That is not a valid Pilotfile");
    vec[0].clone()
}

fn main() {
    match args().nth(1) {
        Some(string) => {
            if string == "-h" || string == "--help" {
                println!("{}", HELP_TEXT);
            } else {
                let yaml = load_pilotfile();

                let mut tasks_to_run = vec![];
                let mut quiet_tasks = vec![];
                let mut raw = false;

                let mut args = args().skip(1);

                while let Some(arg) = args.next() {
                    if arg == "-q" || arg == "--quiet" {
                        break;
                    }

                    if arg == "-r" || arg == "--raw" {
                        raw = true;
                        continue;
                    }

                    tasks_to_run.push(arg);
                }

                while let Some(arg) = args.next() {
                    if arg == "-r" || arg == "--raw" {
                        raw = true;
                        continue;
                    }

                    quiet_tasks.push(arg);
                }

                for task in tasks_to_run {
                    cli_run_task(yaml.clone(), task.clone(), task, quiet_tasks.clone(), raw);
                }
            }
        }
        None => {
            let yaml = load_pilotfile();
            cli_list_tasks(&yaml);
        }
    }
}
