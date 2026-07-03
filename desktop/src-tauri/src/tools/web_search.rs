use anyhow::Result;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
            crate::util::char_prefix(&result.content, 300)
        ));
    }

    Ok(output)
}

// ── SearXNG ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
    #[serde(default)]
    answers: Vec<String>,
}

#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    content: Option<String>,
    #[serde(default)]
    engines: Vec<String>,
}

/// Query a local SearXNG instance (http://localhost:8888 by default).
/// Aggregates 230+ search engines for free. Requires SearXNG running locally.
/// If SearXNG is not reachable, returns a clear error so the agent falls back to web_search.
pub async fn searxng_search(query: &str, searxng_url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let search_url = format!(
        "{}/search?q={}&format=json&language=en",
        searxng_url.trim_end_matches('/'),
        urlencoding::encode(query)
    );

    let resp = client
        .get(&search_url)
        .header("User-Agent", "Bow-Agent/1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("SearXNG not reachable at {}: {}. Is it running? Start with: docker run -d -p 8888:8888 searxng/searxng", searxng_url, e))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("SearXNG returned {}", resp.status()));
    }

    let data: SearxngResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("SearXNG response parse error: {}", e))?;

    let mut output = String::new();

    if let Some(answer) = data.answers.first() {
        output.push_str("**Answer:**\n");
        output.push_str(answer);
        output.push_str("\n\n");
    }

    output.push_str(&format!("**Results ({} found):**\n", data.results.len()));
    for (i, r) in data.results.iter().take(8).enumerate() {
        let snippet = r.content.as_deref().unwrap_or("").trim();
        let engines = if r.engines.is_empty() {
            String::new()
        } else {
            format!(" [{}]", r.engines.join(", "))
        };
        output.push_str(&format!(
            "{}. [{}]({}){}\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            engines,
            crate::util::char_prefix(snippet, 250)
        ));
    }

    if output.trim().is_empty() {
        return Ok("SearXNG returned no results for this query.".to_string());
    }

    Ok(output)
}

// ── Jina Reader ───────────────────────────────────────────────────────────────

/// Fetch a URL via Jina Reader (r.jina.ai) and return clean Markdown.
/// Free — no API key required up to 1M tokens/month.
pub async fn jina_read(url: &str) -> Result<String> {
    let jina_url = format!("https://r.jina.ai/{}", url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client
        .get(&jina_url)
        .header("Accept", "text/markdown")
        .header("User-Agent", "Bow-Agent/1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Jina read failed for '{}': {}", url, e))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "Jina returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }

    let text = resp.text().await?;
    // Truncate to keep context manageable
    let out = crate::util::truncate_with_note(&text, 8000);

    Ok(out)
}

// ── Iterative search refinement ───────────────────────────────────────────────

/// Evaluate whether current search results sufficiently answer the original
/// question. Returns either `"DONE: <reason>"` or `"REFINE: <new query>"`.
/// The agent uses the refined query to run another web_search/web_search_deep.
pub async fn search_evaluate(
    original_question: &str,
    current_results_summary: &str,
    llm_base_url: &str,
    llm_model: &str,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let prompt = format!(
        "You are evaluating whether search results sufficiently answer a question.\n\n\
        Original question: {}\n\n\
        Current search results summary:\n{}\n\n\
        Does the summary fully answer the question?\n\
        - If YES: respond with exactly: DONE: <one sentence why it's sufficient>\n\
        - If NO: respond with exactly: REFINE: <a better follow-up search query that fills the gap>\n\
        Respond with ONLY one of those two formats, nothing else.",
        original_question,
        crate::util::char_prefix(current_results_summary, 2000)
    );

    let body = serde_json::json!({
        "model": llm_model,
        "messages": [
            {"role": "user", "content": prompt}
        ],
        "max_tokens": 80,
        "temperature": 0.3,
        "stream": false
    });

    let resp = client
        .post(format!("{}/v1/chat/completions", llm_base_url))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("search_evaluate LLM call failed: {}", e))?;

    let v: serde_json::Value = resp.json().await?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("DONE: Could not evaluate.")
        .trim()
        .to_string();

    Ok(text)
}

// ── Multi-query expansion ─────────────────────────────────────────────────────

/// Generate 2 alternative query phrasings via the local LLM, then run all 3
/// in parallel against Tavily, merge and deduplicate by URL.
pub async fn web_search_deep(
    query: &str,
    api_key: &str,
    llm_base_url: &str,
    llm_model: &str,
) -> Result<String> {
    // Step 1: expand query into variants
    let variants = expand_queries(query, llm_base_url, llm_model).await;

    // Step 2: parallel Tavily searches
    let futures: Vec<_> = variants
        .iter()
        .map(|q| tavily_search(q, api_key))
        .collect();
    let results = join_all(futures).await;

    // Step 3: merge, deduplicate by URL, keep best-ranked
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut merged: Vec<TavilyResult> = Vec::new();
    let mut summary: Option<String> = None;

    for res in results.into_iter().flatten() {
        if summary.is_none() {
            summary = res.answer;
        }
        for r in res.results {
            if seen_urls.insert(r.url.clone()) {
                merged.push(r);
            }
        }
    }

    // Step 4: format output
    let mut output = String::new();
    if let Some(ans) = &summary {
        output.push_str("**Summary:**\n");
        output.push_str(ans);
        output.push_str("\n\n");
    }
    output.push_str(&format!(
        "**Results ({} unique across {} queries):**\n",
        merged.len(),
        variants.len()
    ));
    for (i, r) in merged.iter().take(10).enumerate() {
        output.push_str(&format!(
            "{}. [{}]({})\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            crate::util::char_prefix(&r.content, 300)
        ));
    }

    Ok(output)
}

/// Ask the local LLM to generate 2 alternative phrasings of the query.
/// Returns original + up to 2 variants (always at least 1 element).
async fn expand_queries(
    query: &str,
    llm_base_url: &str,
    llm_model: &str,
) -> Vec<String> {
    let mut queries = vec![query.to_string()];

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();

    let prompt = format!(
        "Generate exactly 2 alternative search queries for the following query. \
        Each on its own line. No numbering, no explanation, just the queries.\n\nQuery: {}",
        query
    );

    let body = serde_json::json!({
        "model": llm_model,
        "messages": [
            {"role": "system", "content": "You are a search query rewriter. Output only the queries, one per line."},
            {"role": "user", "content": prompt}
        ],
        "max_tokens": 100,
        "temperature": 0.6,
        "stream": false
    });

    if let Ok(resp) = client
        .post(format!("{}/v1/chat/completions", llm_base_url))
        .json(&body)
        .send()
        .await
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(text) = v["choices"][0]["message"]["content"].as_str() {
                for line in text.lines() {
                    let q = line.trim().to_string();
                    if !q.is_empty() && q != query {
                        queries.push(q);
                        if queries.len() >= 3 { break; }
                    }
                }
            }
        }
    }

    queries
}

/// Raw Tavily search returning the parsed response struct.
async fn tavily_search(query: &str, api_key: &str) -> Option<TavilyResponse> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;

    let body = TavilyRequest {
        api_key: api_key.to_string(),
        query: query.to_string(),
        search_depth: "basic".to_string(),
        include_answer: true,
        include_images: false,
        max_results: 5,
    };

    let resp = client
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() { return None; }
    resp.json::<TavilyResponse>().await.ok()
}
