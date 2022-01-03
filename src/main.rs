use std::{
    env::args,
    error::Error,
    fs::read_to_string,
    io::{BufRead, BufReader},
    process::{exit, Command},
    sync::atomic::{AtomicU32, Ordering},
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

fn sanitize_me_this_terminal_string_but_please_preserve_the_colors_oh_and_other_reasonable_ansi_escape_sequences_too(
    mut line: String,
) -> String {
    fn do_remove(i: usize, char: char, line: &mut String) -> u32 {
        if char != '\u{1b}' && char != '\u{1B}' {
            return 0;
        }

        let mut iter = line.chars().skip(i + 1);
        let mut removals: u32 = 1;
        while let Some(char) = iter.next() {
            if char == 'm' {
                return 0;
            }
            if char != '[' && char != ';' && !char.is_ascii_digit() {
                removals += 1;

                if char == 'G' {
                    // we assume the G was part of the ansi escape sequence \u{1b}2K\u{1b}[0G
                    // which clears the line and moves the cursor to the front of the line
                    // so we delete the first part of the string too
                    *line = line[(i + removals as usize)..line.len()].to_string();
                    return 1;
                    // if let Some(slice) = line.get((i - 1)..i) {
                    //     println!("{}", slice.to_string());
                    //     if let Ok(index) = slice.parse::<usize>() {
                    //         *line = line[0..(index - 1)].to_string();
                    //     }
                    // }
                }

                break;
            }

            removals += 1;
        }

        for _ in 0..removals {
            line.remove(i);
        }

        removals
    }

    let mut i = 0;
    while let Some(char) = line.get(i..(i + 1)) {
        let removals = do_remove(i, char.chars().nth(0).unwrap().clone(), &mut line);
        if removals == 0 {
            i += 1;
        }
    }
    line
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

    if raw {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .spawn()
            .or_msg(&format!("Failed to run task {}", task_name))
            .wait()
            .or_msg(&format!("Task {} failed", task_name));
    } else {
        let mut std_command = Command::new("sh");
        std_command.arg("-c").arg(command);

        let process =
            PtyProcess::spawn(std_command).or_msg(&format!("Failed to run task {}", task_name));

        if !quiet_tasks.contains(&task_name) {
            BufReader::new(process.get_pty_stream().or_msg("Could not get pty output"))
                .lines()
                .filter_map(|line| line.ok())
                .map(|line| sanitize_me_this_terminal_string_but_please_preserve_the_colors_oh_and_other_reasonable_ansi_escape_sequences_too(line))
                .for_each(|line| {
                    if timestamp {
                        println!(
                            "{} {}{}:\x1b[0m {}",
                            Local::now().format("%H:%M:%S"),
                            color,
                            task_name,
                            line
                        )
                    } else {
                        println!("{}{}:\x1b[0m {}", color, task_name, line);
                    }
                });
        }

        process.wait().or_msg(&format!("Task {} failed", task_name));
    }

    // the process exited
    if timestamp {
        println!(
            "{} {}{}\x1b[0m finished",
            Local::now().format("%H:%M:%S"),
            color,
            task_name
        );
    } else {
        println!("{}{}\x1b[0m finished", color, task_name);
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
    raw: bool,
    timestamp: bool,
) {
    match task.0.as_str().or_msg(NOT_VALID) {
        "shell" => run_shell(
            task.1.as_str().or_msg(NOT_VALID).to_string(),
            task_name,
            quiet_tasks,
            raw,
            timestamp,
        ),
        "task" => {
            let sub_task = task.1.as_str().or_msg(NOT_VALID).to_string();
            cli_run_task(
                all_tasks,
                sub_task.clone(),
                task_prefix + " > " + &sub_task,
                quiet_tasks,
                raw,
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

                threads.push(thread::spawn(move || {
                    run_task(
                        sub_task_tuple,
                        all_tasks_clone,
                        task_prefix_clone,
                        task_name_clone,
                        quiet_tasks_clone,
                        raw,
                        timestamp,
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
                    raw,
                    timestamp,
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
