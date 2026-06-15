use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use console::{style, Term};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

const APP_NAME: &str = "Categorax";
const STORE_DIR: &str = ".categorax";
const STORE_FILE: &str = "tags.json";
const LAUNCH_QUEUE: &str = "launch-queue.txt";
const LAUNCH_LOCK: &str = "launch.lock";

#[derive(Parser)]
#[command(name = "categorax")]
#[command(
    author,
    version,
    about = "A friendly terminal-first file and folder tagging tool."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, value_name = "PATH")]
    path: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the guided Categorax menu.
    Menu {
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Explorer-friendly launcher that gathers multi-select invocations.
    Launch {
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Add one or more tags to selected files or folders.
    Add {
        #[arg(short, long)]
        tag: Vec<String>,
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Remove one or more tags from selected files or folders.
    Remove {
        #[arg(short, long)]
        tag: Vec<String>,
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Set a category for selected files or folders.
    Category {
        category: String,
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Show tags and categories for selected paths.
    List {
        #[arg(short, long, value_name = "PATH")]
        path: Vec<PathBuf>,
    },
    /// Browse a folder tree grouped by tag or category.
    Browse {
        #[arg(short, long, value_name = "ROOT")]
        root: Option<PathBuf>,
    },
    /// Install the Windows Explorer right-click menu for this executable.
    InstallContext,
    /// Remove the Windows Explorer right-click menu.
    UninstallContext,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TagStore {
    version: u32,
    records: BTreeMap<String, TagRecord>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TagRecord {
    tags: BTreeSet<String>,
    category: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone)]
struct Target {
    absolute: PathBuf,
    store_path: PathBuf,
    key: String,
}

#[derive(Debug, Clone)]
struct LibraryItem {
    path: PathBuf,
    record: TagRecord,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{} {}", style("Categorax error:").red().bold(), err);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    configure_terminal_identity();
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Menu { path }) => interactive_menu(resolve_targets(path)?)?,
        Some(Commands::Launch { path }) => explorer_launch(path)?,
        Some(Commands::Add { tag, path }) => {
            ensure_values(&tag, "tag")?;
            mutate_targets(resolve_targets(path)?, |record| {
                for value in &tag {
                    record.tags.insert(clean_label(value));
                }
            })?;
            println!("{}", style("Tags added.").green().bold());
        }
        Some(Commands::Remove { tag, path }) => {
            ensure_values(&tag, "tag")?;
            mutate_targets(resolve_targets(path)?, |record| {
                for value in &tag {
                    record.tags.remove(&clean_label(value));
                }
            })?;
            println!("{}", style("Tags removed.").green().bold());
        }
        Some(Commands::Category { category, path }) => {
            let category = clean_label(&category);
            mutate_targets(resolve_targets(path)?, |record| {
                record.category = if category.is_empty() {
                    None
                } else {
                    Some(category.clone())
                };
            })?;
            println!("{}", style("Category updated.").green().bold());
        }
        Some(Commands::List { path }) => show_targets(&resolve_targets(path)?)?,
        Some(Commands::Browse { root }) => {
            browse_library(root.unwrap_or(std::env::current_dir()?))?
        }
        Some(Commands::InstallContext) => install_context_menu()?,
        Some(Commands::UninstallContext) => uninstall_context_menu()?,
        None => interactive_menu(resolve_targets(cli.path)?)?,
    }
    Ok(())
}

fn interactive_menu(targets: Vec<Target>) -> Result<()> {
    let term = Term::stdout();
    print_banner();

    if targets.is_empty() {
        println!(
            "{}",
            style("No files or folders were selected.").yellow().bold()
        );
        println!("You can still browse a library, install the Explorer menu, or open help.\n");
    } else {
        print_selected(&targets);
    }

    loop {
        let choice = choose(
            "What would you like to do?",
            &[
                "Add tags",
                "Remove tags",
                "Set category",
                "View current details",
                "Browse by tag/category",
                "Install Windows right-click menu",
                "Help",
                "Exit",
            ],
        )?;

        match choice {
            1 => add_tags_flow(&targets)?,
            2 => remove_tags_flow(&targets)?,
            3 => set_category_flow(&targets)?,
            4 => show_targets(&targets)?,
            5 => browse_flow(&targets)?,
            6 => install_context_menu()?,
            7 => print_help(),
            8 | 0 => {
                println!(
                    "{}",
                    style("Done. Your files are a little easier to find now.")
                        .cyan()
                        .bold()
                );
                break;
            }
            _ => println!(
                "{}",
                style("Please choose one of the shown numbers.").yellow()
            ),
        }

        println!();
        term.write_line(&style("Press Enter to continue...").dim().to_string())?;
        let _ = read_line()?;
        print_banner();
        if !targets.is_empty() {
            print_selected(&targets);
        }
    }

    Ok(())
}

fn add_tags_flow(targets: &[Target]) -> Result<()> {
    require_targets(targets)?;
    let suggestions = collect_nearby_tags(targets)?;
    println!("{}", style("Suggested tags").cyan().bold());
    print_numbered_values(&suggestions);
    println!("Type tag numbers and/or new tag names, separated by commas.");
    println!("{}", style("Example: 1, 3, family photos, tax").dim());
    let answer = prompt("Tags to add")?;
    let tags = parse_numbered_input(&answer, &suggestions);
    if tags.is_empty() {
        println!("{}", style("No tags were added.").yellow());
        return Ok(());
    }
    mutate_targets(targets.to_vec(), |record| {
        for tag in &tags {
            record.tags.insert(tag.clone());
        }
    })?;
    println!("{} {}", style("Added:").green().bold(), tags.join(", "));
    Ok(())
}

fn remove_tags_flow(targets: &[Target]) -> Result<()> {
    require_targets(targets)?;
    let current = collect_current_tags(targets)?
        .into_iter()
        .collect::<Vec<_>>();
    if current.is_empty() {
        println!("{}", style("These items do not have tags yet.").yellow());
        return Ok(());
    }
    println!("{}", style("Current tags").cyan().bold());
    print_numbered_values(&current);
    println!("Type tag numbers and/or exact tag names to remove, separated by commas.");
    let answer = prompt("Tags to remove")?;
    let tags = parse_numbered_input(&answer, &current);
    if tags.is_empty() {
        println!("{}", style("No tags were removed.").yellow());
        return Ok(());
    }
    mutate_targets(targets.to_vec(), |record| {
        for tag in &tags {
            record.tags.remove(tag);
        }
    })?;
    println!("{} {}", style("Removed:").green().bold(), tags.join(", "));
    Ok(())
}

fn set_category_flow(targets: &[Target]) -> Result<()> {
    require_targets(targets)?;
    let suggestions = collect_nearby_categories(targets)?;
    println!("{}", style("Suggested categories").cyan().bold());
    print_numbered_values(&suggestions);
    println!("Type a category number or a new category name. Leave empty to clear.");
    let answer = prompt("Category")?;
    let category = if answer.trim().is_empty() {
        None
    } else if let Ok(index) = answer.trim().parse::<usize>() {
        suggestions.get(index.saturating_sub(1)).cloned()
    } else {
        Some(clean_label(&answer))
    };

    mutate_targets(targets.to_vec(), |record| {
        record.category = category.clone().filter(|value| !value.is_empty());
    })?;
    match category {
        Some(value) => println!("{} {}", style("Category set:").green().bold(), value),
        None => println!("{}", style("Category cleared.").green().bold()),
    }
    Ok(())
}

fn browse_flow(targets: &[Target]) -> Result<()> {
    let default_root = targets
        .first()
        .and_then(|target| target.absolute.parent().map(Path::to_path_buf))
        .unwrap_or(std::env::current_dir()?);
    let answer = prompt_with_default("Folder to browse", &default_root.display().to_string())?;
    browse_library(PathBuf::from(answer))
}

fn browse_library(root: PathBuf) -> Result<()> {
    let root = absolute_path(&root)?;
    let items = scan_library(&root)?;
    if items.is_empty() {
        println!(
            "{} {}",
            style("No Categorax tags found under").yellow().bold(),
            root.display()
        );
        return Ok(());
    }

    let choice = choose(
        "Browse the library by",
        &["Category", "Tag", "Every tagged item", "Back"],
    )?;
    match choice {
        1 => print_grouped(&items, GroupMode::Category),
        2 => print_grouped(&items, GroupMode::Tag),
        3 => print_items(&items),
        _ => {}
    }
    Ok(())
}

enum GroupMode {
    Category,
    Tag,
}

fn print_grouped(items: &[LibraryItem], mode: GroupMode) {
    let mut grouped: BTreeMap<String, Vec<&LibraryItem>> = BTreeMap::new();
    for item in items {
        match mode {
            GroupMode::Category => {
                let key = item
                    .record
                    .category
                    .clone()
                    .unwrap_or_else(|| "Uncategorized".to_string());
                grouped.entry(key).or_default().push(item);
            }
            GroupMode::Tag => {
                if item.record.tags.is_empty() {
                    grouped
                        .entry("Untagged but categorized".to_string())
                        .or_default()
                        .push(item);
                } else {
                    for tag in &item.record.tags {
                        grouped.entry(tag.clone()).or_default().push(item);
                    }
                }
            }
        }
    }

    for (group, values) in grouped {
        println!(
            "\n{}",
            style(format!("{} ({})", group, values.len())).cyan().bold()
        );
        for item in values {
            println!("  {} {}", style("->").green(), item.path.display());
        }
    }
}

fn print_items(items: &[LibraryItem]) {
    for item in items {
        println!("\n{}", style(item.path.display()).bold());
        print_record(&item.record);
    }
}

fn show_targets(targets: &[Target]) -> Result<()> {
    require_targets(targets)?;
    for target in targets {
        let record = read_record(target)?;
        println!("\n{}", style(target.absolute.display()).bold());
        print_record(&record);
    }
    Ok(())
}

fn print_record(record: &TagRecord) {
    let tags = if record.tags.is_empty() {
        style("none").dim().to_string()
    } else {
        record.tags.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    let category = record
        .category
        .clone()
        .unwrap_or_else(|| style("none").dim().to_string());
    println!("  {} {}", style("Category:").cyan(), category);
    println!("  {} {}", style("Tags:").cyan(), tags);
    if let Some(note) = &record.note {
        if !note.trim().is_empty() {
            println!("  {} {}", style("Note:").cyan(), note);
        }
    }
}

fn mutate_targets<F>(targets: Vec<Target>, mut change: F) -> Result<()>
where
    F: FnMut(&mut TagRecord),
{
    require_targets(&targets)?;
    let mut grouped: BTreeMap<PathBuf, Vec<Target>> = BTreeMap::new();
    for target in targets {
        grouped
            .entry(target.store_path.clone())
            .or_default()
            .push(target);
    }

    for (store_path, targets) in grouped {
        let mut store = load_store(&store_path)?;
        for target in targets {
            let record = store.records.entry(target.key).or_default();
            change(record);
        }
        save_store(&store_path, &store)?;
    }
    Ok(())
}

fn read_record(target: &Target) -> Result<TagRecord> {
    let store = load_store(&target.store_path)?;
    Ok(store.records.get(&target.key).cloned().unwrap_or_default())
}

fn resolve_targets(paths: Vec<PathBuf>) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    for path in paths {
        if !path.exists() {
            return Err(anyhow!("Path does not exist: {}", path.display()));
        }
        let absolute = absolute_path(&path)?;
        let parent = absolute.parent().ok_or_else(|| {
            anyhow!(
                "Cannot tag a filesystem root directly: {}",
                absolute.display()
            )
        })?;
        let key = absolute
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("Path has no usable file name: {}", absolute.display()))?
            .to_string();
        let store_path = parent.join(STORE_DIR).join(STORE_FILE);
        targets.push(Target {
            absolute,
            store_path,
            key,
        });
    }
    targets.sort_by(|a, b| a.absolute.cmp(&b.absolute));
    targets.dedup_by(|a, b| a.absolute == b.absolute);
    Ok(targets)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(dunce::canonicalize(path).with_context(|| format!("Cannot read {}", path.display()))?)
    } else if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn load_store(path: &Path) -> Result<TagStore> {
    if !path.exists() {
        return Ok(TagStore {
            version: 1,
            records: BTreeMap::new(),
        });
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut store: TagStore =
        serde_json::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))?;
    if store.version == 0 {
        store.version = 1;
    }
    Ok(store)
}

fn save_store(path: &Path, store: &TagStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(store)?;
    fs::write(path, text).with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(())
}

fn collect_nearby_tags(targets: &[Target]) -> Result<Vec<String>> {
    let mut values = collect_current_tags(targets)?;
    for target in targets {
        if let Some(parent) = target.absolute.parent() {
            let store = load_store(&parent.join(STORE_DIR).join(STORE_FILE))?;
            for record in store.records.values() {
                values.extend(record.tags.iter().cloned());
            }
        }
    }
    Ok(values.into_iter().collect())
}

fn collect_current_tags(targets: &[Target]) -> Result<BTreeSet<String>> {
    let mut values = BTreeSet::new();
    for target in targets {
        values.extend(read_record(target)?.tags);
    }
    Ok(values)
}

fn collect_nearby_categories(targets: &[Target]) -> Result<Vec<String>> {
    let mut values = BTreeSet::new();
    for target in targets {
        if let Some(parent) = target.absolute.parent() {
            let store = load_store(&parent.join(STORE_DIR).join(STORE_FILE))?;
            for record in store.records.values() {
                if let Some(category) = &record.category {
                    if !category.trim().is_empty() {
                        values.insert(category.clone());
                    }
                }
            }
        }
        if let Some(category) = read_record(target)?.category {
            values.insert(category);
        }
    }
    Ok(values.into_iter().collect())
}

fn scan_library(root: &Path) -> Result<Vec<LibraryItem>> {
    let mut items = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() || entry.file_name() != STORE_FILE {
            continue;
        }
        let path = entry.path();
        let Some(store_dir) = path.parent() else {
            continue;
        };
        if store_dir.file_name().and_then(|value| value.to_str()) != Some(STORE_DIR) {
            continue;
        }
        let Some(base_dir) = store_dir.parent() else {
            continue;
        };
        let store = load_store(path)?;
        for (key, record) in store.records {
            if record.tags.is_empty() && record.category.is_none() && record.note.is_none() {
                continue;
            }
            items.push(LibraryItem {
                path: base_dir.join(key),
                record,
            });
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(items)
}

fn explorer_launch(paths: Vec<PathBuf>) -> Result<()> {
    let project_dirs = app_dirs()?;
    let cache_dir = project_dirs.cache_dir();
    fs::create_dir_all(cache_dir)?;
    let queue_path = cache_dir.join(LAUNCH_QUEUE);
    let lock_path = cache_dir.join(LAUNCH_LOCK);

    if !paths.is_empty() {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&queue_path)?;
        for path in paths {
            writeln!(file, "{}", path.display())?;
        }
    }

    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path);
    if lock.is_err() {
        return Ok(());
    }

    thread::sleep(Duration::from_millis(550));
    let queued = fs::read_to_string(&queue_path).unwrap_or_default();
    let _ = fs::remove_file(&queue_path);
    let _ = fs::remove_file(&lock_path);
    let targets = queued
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    interactive_menu(resolve_targets(targets)?)
}

fn configure_terminal_identity() {
    set_terminal_title(APP_NAME);
}

fn set_terminal_title(title: &str) {
    if !io::stdout().is_terminal() {
        return;
    }
    print!("\x1b]0;{}\x07", title);
    let _ = io::stdout().flush();
}

fn app_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("dev", "Categorax", "Categorax")
        .ok_or_else(|| anyhow!("Cannot locate a user data directory for Categorax"))
}

#[cfg(windows)]
fn install_context_menu() -> Result<()> {
    let exe = std::env::current_exe()?.display().to_string();
    let command = windows_terminal_command(&exe);
    reg_add(r"HKCU\Software\Classes\*\shell\Categorax", "", "Categorax")?;
    reg_add(r"HKCU\Software\Classes\*\shell\Categorax", "Icon", &exe)?;
    reg_add(
        r"HKCU\Software\Classes\*\shell\Categorax",
        "MultiSelectModel",
        "Player",
    )?;
    reg_add(
        r"HKCU\Software\Classes\*\shell\Categorax\command",
        "",
        &command,
    )?;

    reg_add(
        r"HKCU\Software\Classes\Directory\shell\Categorax",
        "",
        "Categorax",
    )?;
    reg_add(
        r"HKCU\Software\Classes\Directory\shell\Categorax",
        "Icon",
        &exe,
    )?;
    reg_add(
        r"HKCU\Software\Classes\Directory\shell\Categorax",
        "MultiSelectModel",
        "Player",
    )?;
    reg_add(
        r"HKCU\Software\Classes\Directory\shell\Categorax\command",
        "",
        &command,
    )?;

    println!(
        "{}",
        style("Categorax was added to the Windows right-click menu.")
            .green()
            .bold()
    );
    println!(
        "{}",
        style(
            "Tip: if Explorer is already open, the new menu may appear after reopening the folder."
        )
        .dim()
    );
    Ok(())
}

#[cfg(windows)]
fn windows_terminal_command(exe: &str) -> String {
    if Command::new("where")
        .args(["wt.exe"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
    {
        format!(
            "wt.exe --title \"Categorax\" \"{}\" launch --path \"%1\"",
            exe
        )
    } else {
        format!("\"{}\" launch --path \"%1\"", exe)
    }
}

#[cfg(windows)]
fn uninstall_context_menu() -> Result<()> {
    let _ = Command::new("reg")
        .args(["delete", r"HKCU\Software\Classes\*\shell\Categorax", "/f"])
        .status();
    let _ = Command::new("reg")
        .args([
            "delete",
            r"HKCU\Software\Classes\Directory\shell\Categorax",
            "/f",
        ])
        .status();
    println!(
        "{}",
        style("Categorax was removed from the Windows right-click menu.")
            .green()
            .bold()
    );
    Ok(())
}

#[cfg(windows)]
fn reg_add(key: &str, value_name: &str, value: &str) -> Result<()> {
    let mut args = vec!["add", key];
    if value_name.is_empty() {
        args.push("/ve");
    } else {
        args.extend(["/v", value_name]);
    }
    args.extend(["/d", value, "/f"]);
    let status = Command::new("reg").args(args).status()?;
    if !status.success() {
        return Err(anyhow!("Failed to update Windows registry key: {}", key));
    }
    Ok(())
}

#[cfg(not(windows))]
fn install_context_menu() -> Result<()> {
    println!(
        "{}",
        style("Explorer integration is available on Windows.")
            .yellow()
            .bold()
    );
    println!("On macOS and Linux, run Categorax directly from the terminal:");
    println!("  categorax menu --path /path/to/file");
    Ok(())
}

#[cfg(not(windows))]
fn uninstall_context_menu() -> Result<()> {
    println!(
        "{}",
        style("No Windows Explorer menu is installed on this platform.")
            .yellow()
            .bold()
    );
    Ok(())
}

fn print_banner() {
    println!();
    println!(
        "{}",
        style(format!("  {}", APP_NAME.to_uppercase()))
            .cyan()
            .bold()
    );
    println!(
        "{}",
        style("  Friendly tags and categories for files and folders").dim()
    );
    println!(
        "{}",
        style("  -----------------------------------------------").dim()
    );
}

fn print_selected(targets: &[Target]) {
    println!("{}", style("Selected items").cyan().bold());
    for (index, target) in targets.iter().enumerate() {
        println!("  {}. {}", index + 1, target.absolute.display());
    }
    println!();
}

fn choose(title: &str, options: &[&str]) -> Result<usize> {
    println!("{}", style(title).cyan().bold());
    for (index, option) in options.iter().enumerate() {
        println!("  {}. {}", index + 1, option);
    }
    println!("  0. Back/Exit");
    loop {
        let answer = prompt("Choose a number")?;
        if let Ok(value) = answer.trim().parse::<usize>() {
            if value == 0 || value <= options.len() {
                return Ok(value);
            }
        }
        println!(
            "{}",
            style("That option is not on the menu. Try one of the visible numbers.").yellow()
        );
    }
}

fn prompt(label: &str) -> Result<String> {
    print!("{} ", style(format!("{}:", label)).green().bold());
    io::stdout().flush()?;
    read_line()
}

fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    print!(
        "{} {} ",
        style(format!("{}:", label)).green().bold(),
        style(format!("[{}]", default)).dim()
    );
    io::stdout().flush()?;
    let answer = read_line()?;
    if answer.trim().is_empty() {
        Ok(default.to_string())
    } else {
        Ok(answer)
    }
}

fn read_line() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn print_numbered_values(values: &[String]) {
    if values.is_empty() {
        println!(
            "  {}",
            style("No suggestions yet. Type a new value below.").dim()
        );
    } else {
        for (index, value) in values.iter().enumerate() {
            println!("  {}. {}", index + 1, value);
        }
    }
}

fn parse_numbered_input(answer: &str, suggestions: &[String]) -> Vec<String> {
    let mut values = BTreeSet::new();
    for part in answer.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Ok(index) = token.parse::<usize>() {
            if let Some(value) = suggestions.get(index.saturating_sub(1)) {
                values.insert(value.clone());
                continue;
            }
        }
        let cleaned = clean_label(token);
        if !cleaned.is_empty() {
            values.insert(cleaned);
        }
    }
    values.into_iter().collect()
}

fn clean_label(value: &str) -> String {
    value
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_values(values: &[String], name: &str) -> Result<()> {
    if values.iter().all(|value| clean_label(value).is_empty()) {
        return Err(anyhow!("Please provide at least one {}.", name));
    }
    Ok(())
}

fn require_targets<T>(targets: &[T]) -> Result<()> {
    if targets.is_empty() {
        return Err(anyhow!("Please select at least one file or folder."));
    }
    Ok(())
}

fn print_help() {
    println!("{}", style("How Categorax works").cyan().bold());
    println!("Categorax stores tags beside your files in small .categorax/tags.json files.");
    println!("That makes the library easy to copy, backup, and inspect.");
    println!();
    println!("{}", style("Useful commands").cyan().bold());
    println!("  categorax menu --path \"C:\\Photos\\Summer\"");
    println!("  categorax add --tag vacation --tag family --path \"C:\\Photos\\Summer\"");
    println!("  categorax category Work --path report.docx");
    println!("  categorax browse --root \"C:\\\"");
    println!("  categorax install-context");
}
