use std::process::ExitCode;

use ansi_term::Color::{Cyan, Fixed, Green};
use anyhow::Result;
use clap::Parser;
use sshx::{controller::Controller, runner::Runner, terminal::get_default_shell};
use tokio::signal;
use tracing::error;

/// A secure web-based, collaborative terminal.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Address of the remote sshx server.
    #[clap(long, default_value = "https://sshx.io", env = "SSHX_SERVER")]
    server: String,

    /// Local shell command to run in the terminal.
    #[clap(long)]
    shell: Option<String>,

    /// Quiet mode, only prints the URL to stdout.
    #[clap(short, long)]
    quiet: bool,

    /// Session name displayed in the title (defaults to user@hostname).
    #[clap(long)]
    name: Option<String>,

    /// Enable read-only access mode - generates separate URLs for viewers and
    /// editors.
    #[clap(long)]
    enable_readers: bool,

    // --- Notexo integration ---
    /// Notexo slug for the note to update (optional).
    #[clap(long, env = "NOTEXO_SLUG")]
    notexo_slug: Option<String>,

    /// Notexo note ID (optional, required with slug).
    #[clap(long, env = "NOTEXO_NOTE_ID")]
    notexo_note_id: Option<String>,

    /// Notexo namespace (default: "public").
    #[clap(long, default_value = "public", env = "NOTEXO_NAMESPACE")]
    notexo_namespace: String,
}

fn print_greeting(shell: &str, controller: &Controller) {
    let version_str = match option_env!("CARGO_PKG_VERSION") {
        Some(version) => format!("v{version}"),
        None => String::from("[dev]"),
    };
    if let Some(write_url) = controller.write_url() {
        println!(
            r#"
  {sshx} {version}

  {arr}  Read-only link: {link_v}
  {arr}  Writable link:  {link_e}
  {arr}  Shell:          {shell_v}
"#,
            sshx = Green.bold().paint("sshx"),
            version = Green.paint(&version_str),
            arr = Green.paint("➜"),
            link_v = Cyan.underline().paint(controller.url()),
            link_e = Cyan.underline().paint(write_url),
            shell_v = Fixed(8).paint(shell),
        );
    } else {
        println!(
            r#"
  {sshx} {version}

  {arr}  Link:  {link_v}
  {arr}  Shell: {shell_v}
"#,
            sshx = Green.bold().paint("sshx"),
            version = Green.paint(&version_str),
            arr = Green.paint("➜"),
            link_v = Cyan.underline().paint(controller.url()),
            shell_v = Fixed(8).paint(shell),
        );
    }
}

/// 将 sshx 会话 URL 发送到 Notexo 笔记（后台任务，不阻塞主流程，哦）。
async fn send_url_to_notexo(
    url: String,
    slug: String,
    note_id: String,
    namespace: String,
) {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "slug": slug,
        "namespace": namespace,
        "content": {
            "type": "doc",
            "content": [
                {
                    "type": "paragraph",
                    "attrs": { "textAlign": null },
                    "content": [
                        {
                            "type": "text",
                            "text": url
                        }
                    ]
                }
            ]
        },
        "noteId": note_id
    });

    match client
        .post("https://notexo.in/api/notes")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                eprintln!(
                    "Warning: Notexo API returned {} for note {}/{}",
                    resp.status(),
                    slug,
                    note_id
                );
            }
        }
        Err(e) => {
            eprintln!("Warning: failed to send URL to Notexo: {}", e);
        }
    }
}

#[tokio::main]
async fn start(args: Args) -> Result<()> {
    let shell = match args.shell {
        Some(shell) => shell,
        None => get_default_shell().await,
    };

    let name = args.name.unwrap_or_else(|| {
        let mut name = whoami::username();
        if let Ok(host) = whoami::fallible::hostname() {
            let host = host.split('.').next().unwrap_or(&host);
            name += "@";
            name += host;
        }
        name
    });

    let runner = Runner::Shell(shell.clone());
    let mut controller = Controller::new(&args.server, &name, runner, args.enable_readers).await?;

    // 决定要发送哪个 URL（优先可写链接，其次只读链接）
    let session_url = controller
        .write_url()
        .unwrap_or_else(|| controller.url())
        .to_string();

    if args.quiet {
        println!("{}", session_url);
    } else {
        print_greeting(&shell, &controller);
    }

    // 如果同时提供了 slug 和 noteId，则在后台将 URL 发送到 Notexo
    if let (Some(slug), Some(note_id)) = (args.notexo_slug.clone(), args.notexo_note_id.clone()) {
        tokio::spawn(send_url_to_notexo(
            session_url,
            slug,
            note_id,
            args.notexo_namespace.clone(),
        ));
    }

    let exit_signal = signal::ctrl_c();
    tokio::pin!(exit_signal);
    tokio::select! {
        _ = controller.run() => unreachable!(),
        Ok(()) = &mut exit_signal => (),
    };
    controller.close().await?;

    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();

    let default_level = if args.quiet { "error" } else { "info" };

    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or(default_level.into()))
        .with_writer(std::io::stderr)
        .init();

    match start(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err:?}");
            ExitCode::FAILURE
        }
    }
}
