use std::{
    env::args,
    error::Error,
    fs::read_to_string,
    io::{BufRead, BufReader},
    process::{exit, Command, Stdio},
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
    thread,
};

use chrono::Local;
use ptyprocess::PtyProcess;
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
static PADDING: AtomicUsize = AtomicUsize::new(0);

fn sanitize_string(mut line: String) -> String {
    fn do_remove(i: usize, char: char, line: &mut String) -> bool {
        if char != '\u{1b}' && char != '\u{1B}' {
            return false;
        }

        let mut iter = line.chars().skip(i + 1);
        let mut removals: u32 = 1;
        while let Some(char) = iter.next() {
            // it is a coloring sequence, abort
            if char == 'm' {
                return false;
            }
            if char != '[' && char != ';' && !char.is_ascii_digit() {
                removals += 1;

                // we boldly assume the escape sequence wanted to delete the line so we do so
                *line = line[(i + removals as usize)..line.len()].to_string();
                return true;
                // if let Some(slice) = line.get((i - 1)..i) {
                //     println!("{}", slice.to_string());
                //     if let Ok(index) = slice.parse::<usize>() {
                //         *line = line[0..(index - 1)].to_string();
                //     }
                // }
            }

            removals += 1;
        }

        for _ in 0..removals {
            line.remove(i);
        }

        removals > 0
    }

    let mut i = 0;
    while let Some(char) = line.get(i..(i + 1)) {
        // only increase index if no removals (otherwise there is a new char on the index)
        if do_remove(i, char.chars().nth(0).unwrap().clone(), &mut line) {
            i = 0;
        } else {
            i += 1;
        }
    }
    line
}

#[cfg(target_family = "windows")]
fn get_shell() -> Command {
    let mut command = Command::new(r"C:\Windows\System32\powershell.exe");
    command.arg("-c");
    command
}

#[cfg(not(target_family = "windows"))]
fn get_shell() -> Command {
    use std::env;

    let shell = env::var("SHELL").unwrap_or("sh".to_string());
    let mut command = Command::new(shell);
    command.arg("-c");
    command
}

fn run_shell(
    command: String,
    task_name: String,
    quiet_tasks: Vec<String>,
    raw: bool,
    timestamp: bool,
) {
    // cycle through shell colors
    // credit: https://github.com/chrismytton/shoreman/
    let current_index = INDEX.fetch_add(1, Ordering::SeqCst);
    let color = "\x1b[0;".to_string() + &(31 + current_index % 7).to_string() + "m";

    let mut std_command = get_shell();
    std_command.arg(command);

    let quiet = quiet_tasks.contains(&task_name);

    if raw {
        if quiet {
            std_command.stdout(Stdio::null());
            std_command.stderr(Stdio::null());
        }

        std_command
            .spawn()
            .or_msg(&format!("Failed to run task {}", task_name))
            .wait()
            .or_msg(&format!("Task {} failed", task_name));
    } else {
        let process =
            PtyProcess::spawn(std_command).or_msg(&format!("Failed to run task {}", task_name));

        if !quiet {
            let this_padding = task_name.len() + 1;

            PADDING.fetch_max(this_padding, Ordering::SeqCst);

            BufReader::new(process.get_pty_stream().or_msg("Could not get pty output"))
                .lines()
                .filter_map(|line| line.ok())
                .map(|line| sanitize_string(line))
                .for_each(|line| {
                    let mut time_prefix = "".to_string();

                    if timestamp {
                        time_prefix = Local::now().format("%H:%M:%S").to_string() + " ";
                    }

                    let padding = PADDING.load(Ordering::SeqCst);
                    let padding_prefix = " ".repeat(padding.saturating_sub(this_padding));

                    println!(
                        "{}{}{}:\x1b[0m{} {}",
                        time_prefix, color, task_name, padding_prefix, line
                    );
                });
        }

        process.wait().or_msg(&format!("Task {} failed", task_name));
    }

    // subtract one from the index
    INDEX.fetch_sub(1, Ordering::SeqCst);
}

fn run_task(
    task: (Yaml, Yaml),
    all_tasks: Yaml,
    task_prefix: String,
    task_name: String,
    quiet_tasks: Vec<String>,
    raw: &mut bool,
    timestamp: bool,
) {
    match task.0.as_str().or_msg(NOT_VALID) {
        "shell" => run_shell(
            task.1.as_str().or_msg(NOT_VALID).to_string(),
            task_name,
            quiet_tasks,
            raw.clone(),
            timestamp,
        ),
        "task" => {
            let sub_task = task.1.as_str().or_msg(NOT_VALID).to_string();
            cli_run_task(
                all_tasks,
                sub_task.clone(),
                task_prefix + " > " + &sub_task,
                quiet_tasks,
                raw.clone(),
                timestamp,
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

                let mut raw_clone = raw.clone();

                threads.push(thread::spawn(move || {
                    run_task(
                        sub_task_tuple,
                        all_tasks_clone,
                        task_prefix_clone,
                        task_name_clone,
                        quiet_tasks_clone,
                        &mut raw_clone,
                        timestamp,
                    );
                }));
            }

            for thread in threads {
                thread.join().unwrap();
            }
        }
        "raw" => {
            *raw = task.1.as_bool().or_msg(NOT_VALID);
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
    mut raw: bool,
    timestamp: bool,
) {
    if timestamp {
        println!("{} > {}", Local::now().format("%H:%M:%S"), task_prefix);
    } else {
        println!("> {}", task_prefix);
    }

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
                    &mut raw,
                    timestamp,
                );
            }

            // the process exited
            if timestamp {
                println!(
                    "{} finished {}",
                    Local::now().format("%H:%M:%S"),
                    task_prefix
                );
            } else {
                println!("finished {}", task_prefix);
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
    println!("Available tasks:");

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
                let mut timestamp = false;

                let mut args = args().skip(1);

                while let Some(arg) = args.next() {
                    if arg == "-q" || arg == "--quiet" {
                        break;
                    }

                    if arg == "-r" || arg == "--raw" {
                        raw = true;
                        continue;
                    }

                    if arg == "-t" || arg == "--timestamp" {
                        timestamp = true;
                        continue;
                    }

                    tasks_to_run.push(arg);
                }

                while let Some(arg) = args.next() {
                    if arg == "-r" || arg == "--raw" {
                        raw = true;
                        continue;
                    }

                    if arg == "-t" || arg == "--timestamp" {
                        timestamp = true;
                        continue;
                    }

                    quiet_tasks.push(arg);
                }

                for task in tasks_to_run {
                    cli_run_task(
                        yaml.clone(),
                        task.clone(),
                        task,
                        quiet_tasks.clone(),
                        raw,
                        timestamp,
                    );
                }
            }
        }
        None => {
            let yaml = load_pilotfile();
            cli_list_tasks(&yaml);
        }
    }
}
