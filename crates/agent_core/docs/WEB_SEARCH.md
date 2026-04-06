# Web Search Tools

The agent has two tools for accessing information from the internet:

- **`web_search`** - Search the web and get previews
- **`html_to_text`** - Extract full content from specific URLs

## web_search

Search the web using DuckDuckGo and return results with title, description, URL, and content preview.

### Parameters

- **`query`** (required, string): The search query
- **`limit`** (optional, integer): Maximum number of results to return
    - Default: 10
    - Recommended: 3-7 for focused searches
    - Use fewer (3-5) for specific queries
    - Use more (7-10) for comprehensive coverage
- **`site`** (optional, array of strings, ADVANCED): Specific domains to search within
    - ⚠️ **Use with caution** - Only when you know exactly which sites to search
    - Always provide MULTIPLE related domains (not just one)
    - Examples: `["rust-lang.org", "docs.rs", "blog.rust-lang.org"]`

### ⚠️ Important: When to Use `site` Parameter

**ONLY use `site` when:**

- You know EXACTLY which authoritative sites contain the information
- You can list MULTIPLE related domains to ensure comprehensive coverage
- You are certain these are the canonical/official sources
- The user explicitly requested specific sites

**DO NOT use `site` if:**

- You're not certain about the authoritative domains
- You're doing exploratory searching
- You might miss important information from other sources
- You're searching for general information

**In most cases, use only `query` and `limit`** - let DuckDuckGo find the best sources.

### Content Preview

Each search result includes a **2000 character preview** of the page content:

- Enough to understand what the page contains
- Helps decide if you need to read the full content
- For complete content, use `html_to_text` with the URL

### Examples

**Basic search (MOST COMMON):**

```json
{
    "query": "rust async programming tutorial",
    "limit": 5
}
```

**Advanced search with site filter (use sparingly):**

```json
{
    "query": "async trait implementation",
    "limit": 5,
    "site": ["rust-lang.org", "docs.rs", "blog.rust-lang.org"]
}
```

### Returns

Array of results, each containing:

- `title`: Page title
- `description`: Meta description from search result
- `url`: Full URL
- `content`: First 2000 characters of page content (HTML converted to text)

## html_to_text

Extract readable text content from a specific URL by converting HTML to plain text.

### Parameters

- **`url`** (required, string): The URL to fetch and convert
- **`max_content_length`** (optional, integer): Maximum characters to extract
    - Default: 10000
    - Quick reads: 5000-7000
    - Detailed analysis: 10000-15000
    - Very long documents: Consider multiple calls

### Example

```json
{
    "url": "https://tokio.rs/tokio/tutorial",
    "max_content_length": 10000
}
```

### Returns

Object containing:

- `url`: The URL that was fetched
- `content`: The extracted text content (HTML converted to plain text)

## Recommended Workflows

### Standard Approach (Use This Most of the Time)

1. **Search** with basic parameters - let DuckDuckGo find best sources

    ```json
    {
        "query": "tokio runtime configuration",
        "limit": 5
    }
    ```

2. **Review** the 2000-char previews

3. **Deep read** specific URLs with `html_to_text`
    ```json
    {
        "url": "https://tokio.rs/tokio/topics/runtime",
        "max_content_length": 10000
    }
    ```

### Advanced Approach (Only When You Know What You're Doing)

When you're certain about authoritative sources:

```json
{
    "query": "async fn trait bounds",
    "limit": 5,
    "site": ["rust-lang.org", "doc.rust-lang.org", "docs.rs"]
}
```

**Key requirements for using `site`:**

- Multiple related domains (never just one)
- Certainty about authoritative sources
- Aware you might miss other valuable content

## Example Scenarios

### Quick Information Lookup (Standard)

```json
{
    "query": "rust tokio vs async-std comparison",
    "limit": 3
}
```

The 2000 char previews are usually sufficient.

### Official Documentation (Advanced)

```json
{
    "query": "tokio select macro",
    "limit": 4,
    "site": ["tokio.rs", "docs.rs"]
}
```

Then deep read:

```json
{
    "url": "https://tokio.rs/tokio/tutorial/select",
    "max_content_length": 12000
}
```

### Research Multiple Sources (Standard)

```json
{
    "query": "rust error handling best practices",
    "limit": 7
}
```

Review previews, then use `html_to_text` on top 2-3 results.

## Best Practices

1. **Default to simple searches** - Use only `query` and `limit` in most cases
2. **Use `site` sparingly** - Only when absolutely certain about authoritative domains
3. **Multiple domains required** - Never use `site` with just one domain
4. **Preview first, read second** - web_search → html_to_text workflow
5. **Adjust `limit` wisely** - Don't request more results than needed
6. **Good queries > filters** - A well-crafted query is often better than site filtering

## Implementation Details

### Search Backend

- Uses DuckDuckGo HTML interface
- No API key required
- Privacy-friendly (no tracking)

### Content Processing

- HTML converted to plain text using `html2text`
- Line width: 80 characters
- Preserves structure and readability
- Parallel fetching with `rayon` for speed

### Context Management

**web_search:**

- 2000 chars/page × 10 results = ~20,000 chars max
- Approximately ~5,000 tokens for full default search
- Fixed limit prevents context overload

**html_to_text:**

- Configurable up to ~15,000 characters recommended
- Approximately ~4,000 tokens at 10,000 chars
- Use judiciously for important pages

**Combined Strategy:**

- web_search (5 results): ~2,500 tokens
- html_to_text (1 page): ~2,500 tokens
- Total: ~5,000 tokens for search + deep read

## Error Handling

Both tools gracefully handle errors:

- Network failures return error messages
- Invalid URLs return error descriptions
- HTTP errors (404, 500, etc.) are reported
- Content extraction failures provide fallback messages

## Key Reminder

**The `site` parameter is powerful but dangerous.** Use it only when you know exactly what you're doing. In most cases, a good search query without site filtering will give you better, more comprehensive results.
