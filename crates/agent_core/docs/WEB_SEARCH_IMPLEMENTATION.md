# Web Search Implementation Summary

## Overview

Web search implementation with carefully balanced parameters:
- **`web_search`** - Search with previews (2000 chars/page)
- **`html_to_text`** - Full content extraction (configurable)

## Parameters

### web_search
- `query` (required) - The search query
- `limit` (optional, default: 10) - Number of results
- `site` (optional, ADVANCED) - Array of domains to search within

### html_to_text
- `url` (required) - URL to fetch
- `max_content_length` (optional, default: 10000) - Max characters

## Design Philosophy

### Why Include `site` Parameter?

The `site` parameter is included but with strong warnings because:
1. **Power users need it** - For authoritative source searches
2. **Comprehensive coverage** - Supports multiple domains
3. **Explicit control** - When you know exactly what you need

### Why Strong Warnings?

The parameter is marked ADVANCED and heavily warned because:
1. **Easy to misuse** - Can miss important information
2. **False confidence** - LLMs might think they know sites when they don't
3. **Better alternatives** - Good queries usually work better
4. **Requires certainty** - Only use when absolutely sure

### The Balance

**Most searches should use:**
```json
{
  "query": "rust async programming",
  "limit": 5
}
```

**Only when certain, use:**
```json
{
  "query": "async trait",
  "limit": 5,
  "site": ["rust-lang.org", "docs.rs", "blog.rust-lang.org"]
}
```

## Safety Mechanisms

### 1. Tool Description Warning
```
"ADVANCED: Array of specific domains to search within. Only use if you know 
exactly which authoritative sites to search... DO NOT use unless you are 
certain about the authoritative domains."
```

### 2. System Prompt Warnings

Multiple sections in system_prompt.txt:
- ⚠️ WARNING section listing when to use
- DO NOT use section listing when NOT to use
- Emphasis on "ONLY when you know what you're doing"
- Examples showing both approaches

### 3. Documentation Emphasis

docs/WEB_SEARCH.md includes:
- Dedicated warning section
- Clear DO/DON'T lists
- "Use with caution" messaging
- "Most cases" guidance to avoid it

## Two-Step Workflow

Recommended pattern:

```
1. web_search → Find relevant pages (2000 char previews)
2. html_to_text → Read full content of best matches
```

This balances:
- **Discovery** (web_search with previews)
- **Deep reading** (html_to_text for specifics)
- **Context usage** (preview first, full only when needed)

## Usage Guidelines for LLM

### Standard Approach (90% of cases)
```json
{
  "query": "topic to search",
  "limit": 3-7
}
```

### Advanced Approach (10% of cases)
Only when:
- User explicitly requests specific sites
- You know EXACTLY which authoritative domains
- Multiple related domains provided
- Certain about canonical sources

```json
{
  "query": "specific query",
  "limit": 5,
  "site": ["domain1.com", "domain2.org", "domain3.io"]
}
```

## Implementation Files

```
src/
├── web_search.rs        - Core logic with site filtering
├── tools.rs            - Tool definition with ADVANCED warning
└── main.rs            - Handler supporting array/string for site

docs/
├── WEB_SEARCH.md       - User docs with warnings
├── system_prompt.txt   - LLM instructions with emphasis
└── WEB_SEARCH_IMPLEMENTATION.md  - This file
```

## Context Budget

Example search session:
- web_search (5 results): ~2,500 tokens
- html_to_text (1 page): ~2,500 tokens
- **Total: ~5,000 tokens**

With site filtering:
- Potentially fewer but more focused results
- Same 2000 char/page limit applies
- Same workflow to html_to_text

## Key Principles

1. **Default to simple** - Most searches don't need `site`
2. **Warn heavily** - Multiple layers of warnings
3. **Require multiple domains** - Never just one
4. **Good queries first** - Better search terms > site filtering
5. **User control** - Available when truly needed

## Testing

```bash
cargo check  # ✅ Compiles successfully
```

## The Bottom Line

**The `site` parameter exists for expert use only.** The implementation provides it for power users who need it, but surrounds it with enough warnings that the LLM should rarely use it unless absolutely certain.

Default behavior: Simple searches work best.
Advanced behavior: Available when you know what you're doing.
