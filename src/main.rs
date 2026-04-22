// src/main.rs — SysWatch : Moniteur Système en Réseau

use chrono::Local;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use sysinfo::System;

// ============================================================
// ÉTAPE 1 — Modélisation des données
// ============================================================

#[derive(Debug, Clone)]
struct CpuInfo {
    usage_percent: f32,
    core_count: usize,
}

#[derive(Debug, Clone)]
struct MemInfo {
    total_kb: u64,
    used_kb: u64,
    free_kb: u64,
}

#[derive(Debug, Clone)]
struct ProcessInfo {
    pid: u32,
    name: String,
    cpu_usage: f32,
    mem_kb: u64,
}

#[derive(Debug, Clone)]
struct SystemSnapshot {
    cpu: CpuInfo,
    mem: MemInfo,
    processes: Vec<ProcessInfo>,
    timestamp: String,
}

impl fmt::Display for CpuInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CPU: {:.1}% ({} cœurs)", self.usage_percent, self.core_count)
    }
}

impl fmt::Display for MemInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_mb = self.total_kb / 1024;
        let used_mb  = self.used_kb  / 1024;
        let free_mb  = self.free_kb  / 1024;
        write!(
            f,
            "MEM: {} MB utilisés / {} MB total ({} MB libres)",
            used_mb, total_mb, free_mb
        )
    }
}

impl fmt::Display for ProcessInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "  [{:>6}] {:<25} CPU:{:>5.1}%  MEM:{:>5} MB",
            self.pid, self.name, self.cpu_usage, self.mem_kb / 1024
        )
    }
}

impl fmt::Display for SystemSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "┌─────────────────────────────────────────────┐")?;
        writeln!(f, "│  SysWatch — {}  │", self.timestamp)?;
        writeln!(f, "├─────────────────────────────────────────────┤")?;
        writeln!(f, "│  {}                   ", self.cpu)?;
        writeln!(f, "│  {}   ", self.mem)?;
        writeln!(f, "├─────────────────────────────────────────────┤")?;
        writeln!(f, "│  Top Processus (par CPU)                    │")?;
        for p in &self.processes {
            writeln!(f, "│{}", p)?;
        }
        write!(f, "└─────────────────────────────────────────────┘")
    }
}

// ============================================================
// ÉTAPE 2 — Collecte réelle et gestion d'erreurs
// ============================================================

#[derive(Debug)]
enum SysWatchError {
    CollectionError(String),
}

impl fmt::Display for SysWatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SysWatchError::CollectionError(msg) => write!(f, "Erreur collecte: {}", msg),
        }
    }
}

impl std::error::Error for SysWatchError {}

fn collect_snapshot() -> Result<SystemSnapshot, SysWatchError> {
    let mut sys = System::new_all();
    sys.refresh_all();

    // Double refresh pour des valeurs CPU stables
    thread::sleep(Duration::from_millis(300));
    sys.refresh_all();

    let core_count = sys.cpus().len();
    if core_count == 0 {
        return Err(SysWatchError::CollectionError("Aucun CPU détecté".to_string()));
    }

    let cpu = CpuInfo {
        usage_percent: sys.global_cpu_info().cpu_usage(),
        core_count,
    };

    let mem = MemInfo {
        total_kb: sys.total_memory() / 1024,
        used_kb:  sys.used_memory()  / 1024,
        free_kb:  sys.free_memory()  / 1024,
    };

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .values()
        .map(|p| ProcessInfo {
            pid:       p.pid().as_u32(),
            name:      p.name().to_string(),
            cpu_usage: p.cpu_usage(),
            mem_kb:    p.memory() / 1024,
        })
        .collect();

    // Tri décroissant par CPU, top 5
    processes.sort_by(|a, b| {
        b.cpu_usage
            .partial_cmp(&a.cpu_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    processes.truncate(5);

    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    Ok(SystemSnapshot { cpu, mem, processes, timestamp })
}

// ============================================================
// ÉTAPE 3 — Formatage des réponses réseau
// ============================================================

/// Construit une barre ASCII de `width` caractères proportionnelle à `percent` (0–100).
/// Exemple : 75 % sur 20 chars → ████████████████░░░░
fn ascii_bar(percent: f32, width: usize) -> String {
    let pct    = percent.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f32).round() as usize;
    (0..width).map(|i| if i < filled { '█' } else { '░' }).collect()
}

fn format_response(snapshot: &SystemSnapshot, command: &str) -> String {
    let cmd = command.trim().to_ascii_lowercase();

    match cmd.as_str() {
        "cpu" => {
            let bar = ascii_bar(snapshot.cpu.usage_percent, 20);
            format!(
                "[CPU]\n{}\n[{}] {:.1}%\n",
                snapshot.cpu, bar, snapshot.cpu.usage_percent
            )
        }

        "mem" => {
            let percent = if snapshot.mem.total_kb == 0 {
                0.0_f32
            } else {
                (snapshot.mem.used_kb as f32 / snapshot.mem.total_kb as f32) * 100.0
            };
            let bar = ascii_bar(percent, 20);
            format!(
                "[MEM]\n{}\n[{}] {:.1}%\n",
                snapshot.mem, bar, percent
            )
        }

        "ps" => {
            let body: String = snapshot
                .processes
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{:>2}. {}", i + 1, p))
                .collect::<Vec<_>>()
                .join("\n");
            format!("[PS — Top {} processus]\n{}\n", snapshot.processes.len(), body)
        }

        "all" => {
            format!(
                "{}\n{}\n{}",
                format_response(snapshot, "cpu"),
                format_response(snapshot, "mem"),
                format_response(snapshot, "ps")
            )
        }

        "help" => concat!(
            "Commandes disponibles :\n",
            "  cpu   — Usage CPU + barre ASCII\n",
            "  mem   — Mémoire RAM + barre ASCII\n",
            "  ps    — Top 5 processus par CPU\n",
            "  all   — Vue complète (cpu + mem + ps)\n",
            "  help  — Cette aide\n",
            "  quit  — Fermer la connexion\n",
        ).to_string(),

        "quit" => "Au revoir ! Connexion fermée.\n".to_string(),

        _ => format!(
            "Commande inconnue : '{}'. Tape 'help' pour la liste des commandes.\n",
            command.trim()
        ),
    }
}

// ============================================================
// ÉTAPE 4 — Serveur TCP multi-threadé
// ============================================================

fn snapshot_refresher(shared: Arc<Mutex<SystemSnapshot>>) {
    loop {
        thread::sleep(Duration::from_secs(5));
        match collect_snapshot() {
            Ok(new_snap) => {
                if let Ok(mut snap) = shared.lock() {
                    *snap = new_snap;
                }
            }
            Err(e) => eprintln!("[refresh] Erreur: {}", e),
        }
    }
}

fn handle_client(
    mut stream: TcpStream,
    shared: Arc<Mutex<SystemSnapshot>>,
    log_file: Arc<Mutex<File>>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "inconnu".to_string());

    log_event(&log_file, &format!("CONNECT {}", peer));
    println!("[INFO] Client connecté : {}", peer);

    // Message de bienvenue
    let welcome = concat!(
        "╔══════════════════════════════╗\n",
        "║   SysWatch v1.0 — ENSPD      ║\n",
        "║   Tape 'help' pour commencer ║\n",
        "╚══════════════════════════════╝\n",
        "> "
    );
    let _ = stream.write_all(welcome.as_bytes());

    // Clone du stream pour le BufReader
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[{}] Impossible de cloner le stream: {}", peer, e);
            return;
        }
    };
    let reader = BufReader::new(reader_stream);

    for line in reader.lines() {
        match line {
            Ok(cmd) => {
                let cmd = cmd.trim().to_string();
                if cmd.is_empty() {
                    let _ = stream.write_all(b"> ");
                    continue;
                }

                log_event(&log_file, &format!("CMD {} > {}", peer, cmd));

                if cmd.eq_ignore_ascii_case("quit") {
                    let response = format_response(
                        &shared.lock().unwrap_or_else(|e| e.into_inner()),
                        "quit",
                    );
                    let _ = stream.write_all(response.as_bytes());
                    break;
                }

                let response = match shared.lock() {
                    Ok(snap) => format_response(&snap, &cmd),
                    Err(_)   => "Erreur interne: verrou indisponible\n".to_string(),
                };

                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(b"\n> ");
            }
            Err(_) => break,
        }
    }

    log_event(&log_file, &format!("DISCONNECT {}", peer));
    println!("[INFO] Client déconnecté : {}", peer);
}

// ============================================================
// ÉTAPE 5 (BONUS) — Journalisation fichier
// ============================================================

/// Écrit une entrée horodatée dans le fichier de log partagé.
/// Format : [2025-06-01 14:32:01] CONNECT 127.0.0.1:54321
fn log_event(log_file: &Arc<Mutex<File>>, message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let line = format!("[{}] {}\n", timestamp, message);

    if let Ok(mut file) = log_file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

fn open_log_file() -> Result<File, std::io::Error> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open("syswatch.log")
}

// ============================================================

fn main() {
    println!("[SysWatch] Serveur démarré sur le port 7878");

    // Ouvrir le fichier de log (partagé entre tous les threads)
    let log_file = match open_log_file() {
        Ok(f) => Arc::new(Mutex::new(f)),
        Err(e) => {
            eprintln!("[SysWatch] Impossible d'ouvrir syswatch.log: {}", e);
            return;
        }
    };

    // Collecte initiale
    let initial = match collect_snapshot() {
        Ok(snap) => snap,
        Err(e) => {
            eprintln!("[SysWatch] Erreur collecte initiale: {}", e);
            return;
        }
    };
    println!("Snapshot initial:\n{}\n", initial);

    // Snapshot partagé entre tous les threads
    let shared_snapshot = Arc::new(Mutex::new(initial));

    // Thread de rafraîchissement (toutes les 5 secondes) — démarre AVANT le listener
    {
        let snap_clone = Arc::clone(&shared_snapshot);
        thread::spawn(move || snapshot_refresher(snap_clone));
    }

    // Démarrage du serveur TCP
    let listener = match TcpListener::bind("0.0.0.0:7878") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[SysWatch] Impossible de bind le port 7878: {}", e);
            return;
        }
    };

    println!("Connecte-toi avec : telnet localhost 7878");
    println!("            ou   : nc localhost 7878\n");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let snap_clone = Arc::clone(&shared_snapshot);
                let log_clone  = Arc::clone(&log_file);
                thread::spawn(move || handle_client(stream, snap_clone, log_clone));
            }
            Err(e) => eprintln!("[SysWatch] Erreur connexion entrante: {}", e),
        }
    }
}