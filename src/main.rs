use std::{path::PathBuf, process::exit};

use anyhow::Context;
use async_recursion::async_recursion;
use color_print::{cformat, cprint};
use futures_lite::StreamExt;
use http_body_util::Full;
use hyper::{body::Bytes, Request};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    sync::OnceCell,
    task::JoinHandle,
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
use req::Event;

#[derive(Debug, Clone)]
pub struct Ctx<'c> {
    sys: String,
    functions: Vec<Function>,
    pub contents: Vec<Content>,
    content_parts: Vec<Part>,
    function_parts: Vec<Part>,

    channel: (flume::Sender<String>, flume::Receiver<String>),

    handlebars: handlebars::Handlebars<'c>,
}

impl<'c> Ctx<'c> {
    pub async fn send(&self) -> anyhow::Result<impl futures_lite::Stream<Item = Event>> {
        let data = ChatCompletionRequest {
            contents: self.contents.clone(),
            tools: [Tool::functionDeclarations(self.functions.to_owned())],
            system_instruction: Content {
                parts: vec![Part::text(self.sys.to_owned())],
                role: Default::default(),
            },
        };

        tracing::debug!(name: "req", "raw({})", serde_json::to_string(&data)?);

        use anyhow::Context;
        let req = Request::builder()
            .method(hyper::Method::POST)
            .uri("https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro-latest:streamGenerateContent?alt=sse")
            .header(
                hyper::header::CONTENT_TYPE,
                "application/json",
            )
            .header("x-goog-api-key", std::env::var_os("GOOGLE_API_KEY").context("`GOOGLE_API_KEY` not found!")?.to_string_lossy().as_ref())
            .body(Full::new(Bytes::from(serde_json::to_vec(&data)?)))?;

        Ok(req::send(req).await?)
    }

    pub async fn from<P: AsRef<std::path::Path>>(
        channel: (flume::Sender<String>, flume::Receiver<String>),
        file_path: &P,
    ) -> anyhow::Result<Self> {
        let path = std::path::Path::new(file_path.as_ref());
        let f = tokio::fs::File::open(&path).await?;

        let mut ctx = Self {
            sys: String::new(),
            functions: Vec::new(),
            contents: Vec::new(),
            content_parts: Vec::new(),
            function_parts: Vec::new(),
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

    pub fn is_ended(&self) -> bool {
        if let Some(Content { parts, .. }) = self.contents.last() {
            if parts.iter().any(|part| match part {
                Part::text(s) if s.trim_end().trim_end_matches(".").ends_with("END") => true,
                _ => false,
            }) {
                return true;
            }
        }

        false
    }

    #[async_recursion]
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        while let Some(fn_part) = self.function_parts.pop() {
            match fn_part {
                Part::functionCall { ref name, ref args } => {
                    let (name, args) = (name.to_string(), args.clone());

                    self.contents.push(Content {
                        parts: vec![fn_part],
                        role: "model".into(),
                    });

                    match Ctx::from(
                        self.channel.clone(),
                        &config_dir().await?.join(format!("{name}.module")),
                    )
                    .await
                    {
                        Ok(mut context) => {
                            let q = serde_json::to_string(
                                &args
                                    .get("query")
                                    .and_then(serde_json::Value::as_str)
                                    .context("no query found for context")?,
                            )?;

                            tracing::info!(name: "context", "{}", context.sys);
                            tracing::info!(name: "query", "{}", q);

                            context.set(q);

                            let last = loop {
                                context.tick().await?;

                                if context.is_ended() {
                                    // passes an output of sub-contex
                                    // back to parent.
                                    break context.contents;
                                }

                                let line = self.channel.1.recv_async().await?;
                                context.contents.push(Content::user(line));
                            };

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

                            let (detach, c) = if c.starts_with("#!nowait") {
                                (true, c.replace("#!nowait", "trap \"\" HUP"))
                            } else {
                                (false, c)
                            };

                            let mut cmd = tokio::process::Command::new("sh");

                            let stdout = if detach {
                                cmd.args(["-c", &c]).spawn()?;
                                Default::default()
                            } else {
                                cmd.args(["-c", &c]).output().await?.stdout
                            };

                            tracing::info!(name, args = serde_json::to_string(&args)?);
                            tracing::info!(name, output = std::str::from_utf8(&stdout)?);

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
                _e => unreachable!("{:?}", _e),
            }
        }

        if !self.content_parts.is_empty() {
            self.contents.push(Content {
                parts: self.content_parts.drain(..).collect(),
                role: "model".into(),
            });
        }

        let last_msg = &self.contents.last().expect("failed to get last msg");
        match last_msg {
            Content { parts, role } if role == "model" => {
                tracing::info!(name: "recv", data =?self.contents);

                let str = parts.iter().fold(String::new(), |s, part| match part {
                    Part::text(t) => s + t,
                    Part::inlineData { mimeType, data } => {
                        s + format!("![](data:{mimeType};base64,{data})").as_str()
                    }
                    _ => s,
                });

                self.channel.0.send_async(str).await?;

                return Ok(());
            }

            _ => (),
        }

        let mut stream = self.send().await?;

        while let Some(ChatCompletionResponse {
            candidates: [Candidate { content, .. }],
        }) = stream
            .next()
            .await
            .inspect(|d| {
                tracing::debug!(name: "event", "{}", d);
            })
            .and_then(|next| match next {
                Event::Data(d) => serde_json::from_str(&d).ok(),
                _e => unreachable!("{:?}", _e),
            })
        {
            match content {
                Some(Content { mut parts, .. }) => {
                    while let Some(popped) = parts.pop() {
                        match popped {
                            Part::functionResponse { .. } => unreachable!("{popped:?}"),
                            Part::text(_) | Part::inlineData { .. } => {
                                self.content_parts.push(popped)
                            }
                            Part::functionCall { .. } => self.function_parts.push(popped),
                        }
                    }
                }
                None => anyhow::bail!("no content received"),
            }
        }

        self.tick().await?;

        Ok(())
    }
}

#[tracing::instrument]
pub async fn runner() -> anyhow::Result<(
    JoinHandle<()>,
    (flume::Sender<String>, flume::Receiver<String>),
)> {
    let (tx, _rx) = flume::unbounded::<String>();
    let (_tx, rx) = flume::unbounded::<String>();

    let default_cfg = config_dir().await?;
    let handle = tokio::spawn(async move {
        match Ctx::from(
            (_tx.clone(), _rx.clone()),
            &default_cfg.join("default.module"),
        )
        .await
        {
            Ok(mut context) => {
                while let Ok(msg) = _rx.recv_async().await {
                    context.set(msg);

                    if let Err(err) = context.tick().await {
                        tracing::error!(name: "context", "Failed to run: {err}");
                        break;
                    }

                    if context.is_ended() {
                        drop(context);

                        drop(_tx);
                        drop(_rx);
                        break;
                    }
                }
            }
            Err(err) => tracing::error!(name: "context", "Invalid configuration: {err}"),
        }
    });

    Ok((handle, (tx, rx)))
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

        let default_cfg = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ctx_dir.join("default.module"))
            .await;

        if Some(std::io::ErrorKind::AlreadyExists)
            != default_cfg.as_ref().err().map(std::io::Error::kind)
            || default_cfg.is_ok()
        {
            _ = default_cfg?
                .write_all(
                    b"You are engaged in conversation with user.
You should give clear, complete answers that minimize follow-up questions.
You resolve user provided tasks or queries using available tools/functions.
Anticipate and address related queries, providingnecessary context and deatails.
Avoid ending responses with prompts or questions unless instructed.
Reply 'END' instead of exit lines, farewell remarks or sign-offs at end of conversation.
+++",
                )
                .await?;
        }

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

    let (r, (tx, rx)) = runner().await?;

    let mut stdout = tokio::io::stdout();
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();

    while !r.is_finished() {
        tokio::select! {
            Ok(Some(msg)) = reader.next_line() => {
                _ = tx.send_async(msg).await;
            }
            Ok(msg) = rx.recv_async() => {
                stdout.write(cformat!("<dim>{msg}</>").as_bytes()).await?;

                if r.is_finished() {
                    stdout.write(b"\n").await;
                }

                stdout.flush().await?;
            }
        }
    }

    r.await?;

    exit(0)
}
