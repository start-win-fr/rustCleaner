use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rustcleaner::clean::{CleanOutcome, RecycleBin, execute};
use rustcleaner::paths::Env;
use rustcleaner::performance::running_names as running_processes;
use rustcleaner::rules::{self, Rule};
use rustcleaner::safety::Guard;
use rustcleaner::scan::{RuleStatus, ScanReport, format_size, scan};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Parser)]
#[command(version, about = "Nettoyeur de disque piloté par des règles")]
struct Cli {
    /// Dossier de règles .toml à utiliser à la place des règles intégrées
    #[arg(long, global = true)]
    rules: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Affiche les règles disponibles
    List,
    /// Calcule ce qui serait supprimé, sans rien supprimer
    Scan {
        /// Limite le scan à ces identifiants de règles
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Inclut les règles désactivées par défaut
        #[arg(long)]
        all: bool,
        /// Liste chaque fichier concerné
        #[arg(long)]
        details: bool,
        /// Sortie au format JSON
        #[arg(long)]
        json: bool,
    },
    /// Déplace les fichiers vers la corbeille, après confirmation
    Clean {
        /// Limite le nettoyage à ces identifiants de règles
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Inclut les règles désactivées par défaut
        #[arg(long)]
        all: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rules = match &cli.rules {
        Some(dir) => rules::load_dir(dir)?,
        None => rules::builtin()?,
    };

    match cli.command {
        Command::List => {
            for rule in &rules {
                let state = if rule.enabled_by_default { "actif" } else { "inactif" };
                println!("{:<26} {:<8} {}", rule.id, state, rule.name);
            }
        }
        Command::Scan {
            only,
            all,
            details,
            json,
        } => {
            let selected = select(rules, &only, all)?;
            let mut report = scan(
                &selected,
                &Env::from_process(),
                &Guard::system(),
                &running_processes()?,
                SystemTime::now(),
            );
            if !details {
                for rule in &mut report.rules {
                    rule.files.clear();
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report, details);
                println!(
                    "\nTotal : {} dans {} fichiers. Simulation : aucun fichier n'a été supprimé.",
                    format_size(report.bytes),
                    report.file_count
                );
            }
        }
        Command::Clean { only, all } => {
            let selected = select(rules, &only, all)?;
            let report = scan(
                &selected,
                &Env::from_process(),
                &Guard::system(),
                &running_processes()?,
                SystemTime::now(),
            );
            print_report(&report, false);
            if report.file_count == 0 {
                println!("\nRien à nettoyer.");
                return Ok(());
            }
            println!();
            if !confirm(report.file_count, report.bytes)? {
                println!("Annulé. Aucun fichier n'a été déplacé.");
                return Ok(());
            }
            println!("Déplacement vers la corbeille en cours...");
            // Checked again: a browser may have been opened during the confirmation.
            let outcome = execute(&report, &selected, &running_processes()?, &RecycleBin);
            print_outcome(&outcome);
        }
    }
    Ok(())
}

/// Anything other than the typed word, including end of input, means no.
fn confirm(files: u64, bytes: u64) -> Result<bool> {
    print!(
        "Déplacer {files} fichiers ({}) vers la corbeille ? Tapez « oui » pour confirmer : ",
        format_size(bytes)
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("oui"))
}

fn print_outcome(outcome: &CleanOutcome) {
    println!(
        "Déplacés vers la corbeille : {} fichiers ({}).",
        outcome.moved_files,
        format_size(outcome.moved_bytes)
    );
    for rule in &outcome.blocked_rules {
        println!("Règle ignorée, programme ouvert entre-temps : {rule}");
    }
    if !outcome.skipped.is_empty() {
        println!("Laissés en place, modifiés depuis le scan : {}", outcome.skipped.len());
    }
    if !outcome.failed.is_empty() {
        println!(
            "Non déplacés, souvent parce qu'ils sont en cours d'utilisation : {}",
            outcome.failed.len()
        );
        for problem in outcome.failed.iter().take(10) {
            println!("    {}", problem.path.display());
        }
        if outcome.failed.len() > 10 {
            println!("    ... et {} autres", outcome.failed.len() - 10);
        }
    }
}

fn select(rules: Vec<Rule>, only: &[String], all: bool) -> Result<Vec<Rule>> {
    if only.is_empty() {
        return Ok(rules
            .into_iter()
            .filter(|rule| all || rule.enabled_by_default)
            .collect());
    }
    for id in only {
        if !rules.iter().any(|rule| &rule.id == id) {
            bail!("règle inconnue : {id}");
        }
    }
    Ok(rules
        .into_iter()
        .filter(|rule| only.contains(&rule.id))
        .collect())
}

fn print_report(report: &ScanReport, details: bool) {
    for rule in &report.rules {
        match &rule.status {
            RuleStatus::Scanned => println!(
                "{:<26} {:>10}  {:>8} fichiers  {}",
                rule.id,
                format_size(rule.bytes),
                rule.file_count,
                rule.name
            ),
            RuleStatus::NotDetected => println!("{:<26} non détecté", rule.id),
            RuleStatus::BlockedBy(names) => {
                println!("{:<26} ignoré, en cours d'exécution : {}", rule.id, names.join(", "))
            }
            RuleStatus::Unavailable(reason) => {
                println!("{:<26} indisponible : {reason}", rule.id)
            }
        }
        for refused in &rule.refused {
            println!("    refusé : {refused}");
        }
        if !rule.errors.is_empty() {
            println!("    {} chemin(s) illisible(s)", rule.errors.len());
        }
        if details {
            for file in &rule.files {
                println!("    {}  ({})", file.path.display(), format_size(file.bytes));
            }
        }
    }
}
