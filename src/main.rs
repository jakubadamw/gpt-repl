mod api;
mod readline;

use anyhow::Ok;
use std::env;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use structopt::StructOpt;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Config {
    pub api_key: String,
    pub max_tokens: usize,
    pub model: String,
    pub temperature: f64,
}

fn get_default_config_path() -> PathBuf {
    let package_name = env!("CARGO_PKG_NAME");
    directories::UserDirs::new()
        .unwrap()
        .home_dir()
        .join(".config")
        .join(package_name)
        .with_extension("toml")
}

#[derive(Debug, structopt::StructOpt)]
struct Options {
    /// Path to the configuration file.
    #[structopt(short, long)]
    config_path: Option<PathBuf>,
}

fn load_config(path: impl AsRef<Path>) -> Config {
    let contents = std::fs::read_to_string(path).unwrap();
    toml::from_str(&contents).unwrap()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use futures_util::StreamExt;

    let Options { config_path } = Options::from_args();
    let config_path = config_path.unwrap_or_else(get_default_config_path);
    if !config_path.exists() {
        anyhow::bail!(
            "Config file under {} does not exist.",
            config_path.display()
        );
    }

    let config = load_config(config_path);

    let (line_sender, mut line_receiver) = tokio::sync::mpsc::channel(10);
    let (interrupt_sender, mut interrupt_receiver) = tokio::sync::mpsc::channel(10);
    let (readiness_sender, readliness_receiver) = std::sync::mpsc::channel();

    let http_client = reqwest::Client::new();

    let mut messages = vec![];

    let mut interrupt_count: usize = 0;

    futures_util::try_join! {
        readline::run_readline_loop(line_sender, interrupt_sender, readliness_receiver),
        async {
            let mut stream: Option<Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<String>>>>> = None;
            let mut response: Option<String> = None;
            loop {
                tokio::select! {
                    _ = async {
                        tokio::select! {
                        _ = tokio::signal::ctrl_c() => (),
                        _ = interrupt_receiver.recv() => (),
                        }
                    } => {
                        if interrupt_count > 0 {
                            drop(readiness_sender);
                            break;
                        }
                        interrupt_count += 1;
                        if stream.is_some() {
                            stream = None;
                            messages.pop();
                            if response.as_ref().is_some_and(|string| !string.is_empty()) {
                                println!();
                            }
                        }
                        println!();
                        println!("Interrupt again to exit.");
                        println!();
                        readiness_sender.send(()).unwrap();
                    }
                    Some(chunk) = futures_util::future::OptionFuture::from(stream.as_mut().map(futures_util::TryStreamExt::try_next)) => {
                        if let Some(chunk) = chunk? {
                            *response.as_mut().unwrap() += chunk.as_str();
                            print!("{chunk}");
                            std::io::stdout().flush()?;
                        } else {
                            stream = None;
                            messages.push((api::RequestMessageRole::Assistant, response.take().unwrap()));
                            println!();
                            println!();
                            readiness_sender.send(()).unwrap();
                        }
                    }
                    message = line_receiver.recv() => {
                        let Some(line) = message else {
                            break;
                        };
                        interrupt_count = 0;
                        messages.push((api::RequestMessageRole::User, line));
                        response = Some(String::new());

                        println!();
                        stream = Some(api::request(
                            &http_client,
                            &config,
                            messages.clone()
                        ).boxed());
                    }
                }
            }
            Ok(())
        }
    }?;

    Ok(())
}
