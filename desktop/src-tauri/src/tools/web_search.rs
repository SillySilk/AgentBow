use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    search_depth: String,
    include_answer: bool,
    include_images: bool,
    max_results: u8,
}

#[derive(Deserialize)]
struct TavilyResponse {
    answer: Option<String>,
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    #[allow(dead_code)]
    score: f64,
}

pub async fn web_search(query: &str, api_key: &str) -> Result<String> {
    let client = reqwest::Client::new();

    let request_body = TavilyRequest {
        api_key: api_key.to_string(),
        query: query.to_string(),
        search_depth: "basic".to_string(),
        include_answer: true,
        include_images: true,
        max_results: 5,
    };

    let response = client
        .post("https://api.tavily.com/search")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Tavily request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Tavily API error {}: {}",
            status,
            body
        ));
    }

    let tavily: TavilyResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse Tavily response: {}", e))?;

    let mut output = String::new();

    if let Some(answer) = &tavily.answer {
        output.push_str("**Summary:**\n");
        output.push_str(answer);
        output.push_str("\n\n");
    }

    output.push_str("**Top Results:**\n");
    for (i, result) in tavily.results.iter().enumerate() {
        output.push_str(&format!(
            "{}. [{}]({})\n   {}\n\n",
            i + 1,
            result.title,
            result.url,
            &result.content[..result.content.len().min(300)]
        ));
    }

    Ok(output)
}
