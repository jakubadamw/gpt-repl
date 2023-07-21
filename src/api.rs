#[derive(Debug, serde::Deserialize)]
pub struct Message {
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub delta: Option<Message>,
}

#[derive(Debug, serde::Deserialize)]
pub struct Error {
    pub message: String,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum StreamPatch {
    Success { choices: Vec<Choice> },
    Error { error: Error },
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestMessageRole {
    // System,
    User,
    Assistant,
}

#[derive(serde::Serialize)]
pub struct RequestMessage<'a> {
    pub role: RequestMessageRole,
    pub content: &'a str,
}

#[derive(serde::Serialize)]
pub struct Request<'a> {
    pub model: &'a str,
    pub messages: Vec<RequestMessage<'a>>,
    pub max_tokens: usize,
    pub temperature: f64,
    pub stream: bool,
}

fn merge_newline_split_across_patches(buffer: &mut String, patch: &str) -> String {
    if patch.ends_with('\'') {
        *buffer += patch;
        String::new()
    } else if !buffer.is_empty() {
        let result = format!("{buffer}{patch}").replace("\\n", "\n");
        buffer.clear();
        result
    } else {
        patch.to_string()
    }
}

pub fn request(
    http_client: &reqwest::Client,
    config: &super::Config,
    messages: Vec<(RequestMessageRole, String)>,
) -> impl futures_util::Stream<Item = anyhow::Result<String>> {
    use futures_util::{AsyncBufReadExt, TryStreamExt};

    let api_key = &config.api_key;
    let model = &config.model;
    let max_tokens = config.max_tokens;
    let temperature = config.temperature;

    let reqwest = http_client
        .post("https://api.openai.com/v1/chat/completions")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&Request {
            model,
            messages: messages
                .iter()
                .map(|(role, content)| RequestMessage {
                    role: *role,
                    content: content.as_str(),
                })
                .collect(),
            max_tokens,
            temperature,
            stream: true,
        });

    async_stream::try_stream! {
        let response = reqwest.send().await?;

        let mut buffer = String::new();

        let lines = response
            .bytes_stream()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
            .into_async_read()
            .lines()
            .try_filter_map(|line| futures_util::future::ready(Ok(line.strip_prefix("data: ").map(str::to_string))))
            .try_take_while(|line| futures_util::future::ready(Ok(line != "[DONE]")));
        futures_util::pin_mut!(lines);

        while let Some(line) = lines.try_next().await? {
            match serde_json::from_str::<'_, StreamPatch>(&line) {
                Ok(StreamPatch::Success { choices }) => {
                    if let [choice] = choices.as_slice() {
                        let Some(content) = choice
                            .delta
                            .as_ref()
                            .and_then(|delta| delta.content.as_deref())
                        else {
                            continue;
                        };

                        yield merge_newline_split_across_patches(&mut buffer, content);
                    } else {
                        Err(anyhow::anyhow!("wrong length of choices: {choices:?}"))?;
                    }
                }
                Ok(StreamPatch::Error { error }) => {
                    Err(anyhow::anyhow!("structured error: {error:?}"))?;
                }
                Err(_) => {
                    Err(anyhow::anyhow!("malformed data: {line:?}"))?;
                }
            }
        }
    }
}
