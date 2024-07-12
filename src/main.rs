use std::{path::PathBuf, process::exit};

use anyhow::Context;
use async_recursion::async_recursion;
use color_print::cformat;
use futures_lite::StreamExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    pin,
    sync::OnceCell,
};

#[path = "types/google.rs"]
mod google;

#[path = "types/openapi.rs"]
mod openapi;

mod req;

use google::{
    Candidate, ChatCompletionRequest, ChatCompletionResponse, Content, Function, Part, Response,
    Tool,
};

const DEFAULT_PROMPT: &str = "You are engaged in conversation with user; You must follow following rules during conversation:
You resolve user provided tasks or queries using available tools/function calls.
Anticipate and address related queries, providing necessary context and details.
You should give clear, complete answers that minimize follow-up questions.
Your conversation is limited for one time assistantce so do not ask for more.
Do not repeat. Be short spacific and factual with valid data.
Reply `***` instead of exit lines, farewell remarks or sign-offs at end of conversation.

";

#[derive(Debug, Clone)]
pub struct Ctx<'c> {
    sys: String,
    functions: Vec<Function>,
    pub contents: Vec<Content>,
    is_ended: bool,

    channel: (flume::Sender<String>, flume::Receiver<String>),

    handlebars: handlebars::Handlebars<'c>,
}

impl<'c> Ctx<'c> {
    pub fn new(channel: (flume::Sender<String>, flume::Receiver<String>)) -> anyhow::Result<Self> {
        Ok(Self {
            sys: DEFAULT_PROMPT.into(),
            functions: Vec::new(),
            contents: Vec::new(),
            is_ended: false,
            channel,
            handlebars: handlebars::Handlebars::new(),
        })
    }

    pub async fn from<P: AsRef<std::path::Path>>(
        channel: (flume::Sender<String>, flume::Receiver<String>),
        file_path: &P,
    ) -> anyhow::Result<Self> {
        let path = std::path::Path::new(file_path.as_ref());
        let f = tokio::fs::File::open(&path).await?;

        let mut ctx = Self {
            sys: DEFAULT_PROMPT.into(),
            functions: Vec::new(),
            contents: Vec::new(),
            is_ended: false,
            channel,

            handlebars: handlebars::Handlebars::new(),
        };

        let reader = tokio::io::BufReader::new(f);
        let mut lines = reader.chain(&b"+++"[..]).lines();

        let mut buffer = String::new();
        while let Some(s) = lines.next_line().await? {
            if s.starts_with("+++") {
                let toml = toml::from_str(&buffer);

                if toml.is_err() && ctx.functions.is_empty() {
                    if !buffer.trim().is_empty() {
                        ctx.sys.push('\n');
                        ctx.sys.push_str(&buffer.trim());
                    }

                    buffer.clear();
                    continue;
                }

                let f: Function = toml?;

                ctx.handlebars
                    .register_template_string(&f.name, f.exec.clone())?;

                ctx.functions.push(f);

                buffer.clear()
            } else {
                buffer.push_str(&s.trim());
                buffer.push('\n');
            }
        }

        Ok(ctx)
    }

    pub fn set(&mut self, input: String) {
        self.contents.push(Content::user(input));
    }

    #[async_recursion]
    pub async fn tick(&mut self) -> anyhow::Result<Vec<Content>> {
        let data = ChatCompletionRequest {
            contents: self.contents.clone(),
            tools: [Tool::functionDeclarations(self.functions.to_owned())],
            system_instruction: Content {
                parts: vec![Part::text(self.sys.to_owned())],
                role: Default::default(),
            },
        };

        tracing::debug!(name: "req", "raw({})", serde_json::to_string(&data)?);

        let stream = req::make(&data).await?;

        pin!(stream);

        let mut content_parts = Vec::new();

        while let Some(Ok(ChatCompletionResponse {
            candidates: [Candidate { content, .. }],
        })) = stream
            .next()
            .await
            .inspect(|d| tracing::debug!(name: "event", event=?d))
        {
            match content {
                Some(Content { mut parts, .. }) => {
                    while let Some(part) = parts.pop() {
                        match part {
                            Part::functionCall { ref name, ref args } => {
                                let (name, args) = (name.to_string(), args.clone());

                                self.contents.push(Content {
                                    parts: vec![part],
                                    role: "model".into(),
                                });

                                match Ctx::from(
                                    self.channel.clone(),
                                    &config_dir().await?.join(format!("{name}.module")),
                                )
                                .await
                                {
                                    Ok(mut context) => {
                                        let q = args
                                            .get("input")
                                            .and_then(serde_json::Value::as_str)
                                            .context("no input found for context")?;

                                        tracing::info!(name: "context", ctx=context.sys);
                                        tracing::info!(name: "input", input=q);

                                        context.set(q.into());

                                        let last = context.tick().await?;

                                        self.contents.push(Content {
                                            parts: vec![Part::functionResponse {
                                                name: name.clone(),
                                                response: Response {
                                                    name: name,
                                                    content: serde_json::to_string(&last)?,
                                                },
                                            }],
                                            role: "function".into(),
                                        });
                                    }
                                    _ => {
                                        let c = self.handlebars.render(&name, &args)?;
                                        tracing::debug!(name: "cmd", cmd=?c);

                                        let mut cmd = tokio::process::Command::new("sh");
                                        let stdout = if c.starts_with("#!nowait") {
                                            cmd.args([
                                                "-c",
                                                &c.replace("#!nowait", "trap \"\" HUP"),
                                            ])
                                            .spawn()?;
                                            Default::default()
                                        } else {
                                            cmd.args(["-c", &c]).output().await?.stdout
                                        };

                                        tracing::info!(name, args = serde_json::to_string(&args)?);
                                        tracing::info!(
                                            name,
                                            cmd = ?cmd,
                                            output = std::str::from_utf8(&stdout)?
                                        );

                                        self.contents.push(Content {
                                            parts: vec![Part::functionResponse {
                                                name: name.clone(),
                                                response: Response {
                                                    name: name,
                                                    content: std::str::from_utf8(&stdout)?.into(),
                                                },
                                            }],
                                            role: "function".into(),
                                        });
                                    }
                                }
                            }
                            Part::text(ref t) => {
                                self.channel.0.send_async(t.into()).await?;

                                if t.trim_end().trim_end_matches(".").ends_with("***") {
                                    self.is_ended = true;
                                }

                                content_parts.push(part)
                            }

                            Part::inlineData {
                                ref mimeType,
                                ref data,
                            } => {
                                self.channel
                                    .0
                                    .send_async(format!("![](data:{mimeType};base64,{data})"))
                                    .await?;
                                content_parts.push(part)
                            }
                            _e => unreachable!("{:?}", _e),
                        }
                    }
                }
                // exit on empty response.
                None => return Ok(self.contents.drain(..).collect()),
            }
        }

        if !content_parts.is_empty() {
            self.contents.push(Content {
                parts: content_parts,
                role: "model".into(),
            });
        }

        let last_msg = &self.contents.last().expect("failed to get last msg");

        match last_msg {
            Content { role, .. } if role == "model" => {
                self.contents.retain_mut(|c| {
                    c.parts.retain(|p| match p {
                        Part::text(_) | Part::inlineData { .. } => true,
                        _ => false,
                    });

                    !c.parts.is_empty()
                });

                let c = &self.contents;
                tracing::info!(name: "end", contents=?c);

                Ok(self.contents.drain(..).collect())
            }

            Content { role, .. } if role == "function" => self.tick().await,

            _ => {
                let line = self.channel.1.recv_async().await?;
                self.contents.push(Content::user(line));
                self.tick().await
            }
        }
    }
}

static CTX: OnceCell<PathBuf> = OnceCell::const_new();
async fn config_dir() -> anyhow::Result<&'static PathBuf> {
    use anyhow::Context;

    CTX.get_or_try_init(|| async {
        std::env::var_os("GOOGLE_API_KEY").context("`GOOGLE_API_KEY` not found!")?;

        let ctx_dir = dirs::home_dir()
            .context("failed to get user home")?
            .join(".config")
            .join("ctxrn");

        tokio::fs::create_dir_all(&ctx_dir).await?;

        Ok(ctx_dir)
    })
    .await
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .pretty()
        .with_thread_names(true)
        .init();

    let (tx, _rx) = flume::unbounded::<String>();
    let (_tx, rx) = flume::unbounded::<String>();

    let default_cfg = config_dir().await?;
    let r = tokio::spawn(async move {
        match Ctx::from(
            (_tx.clone(), _rx.clone()),
            &default_cfg.join("default.module"),
        )
        .await
        .or(Ctx::new((_tx.clone(), _rx.clone())))
        {
            Ok(mut context) => {
                while let Ok(msg) = _rx.recv_async().await {
                    context.set(msg);

                    if let Err(err) = context.tick().await {
                        tracing::error!(name: "context", "Failed to run: {err}");
                        break;
                    }

                    if context.is_ended {
                        drop(context);
                        break;
                    }
                }
            }
            Err(err) => tracing::error!(name: "context", "Invalid configuration: {err}"),
        }

        tracing::info!(name: "thread", "ended!");
    });

    let mut stdout = tokio::io::stdout();
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();

    loop {
        tokio::select! {
            Ok(Some(msg)) = reader.next_line() => {
                _ = tx.send_async(msg).await;
            }
            Ok(msg) = rx.recv_async() => {
                stdout.write(cformat!("<dim>{msg}</>").as_bytes()).await?;
                stdout.flush().await?;

                if r.is_finished() {
                    drop(tx);
                    break;
                }

            }
        }
    }

    r.await?;

    exit(0)
}
