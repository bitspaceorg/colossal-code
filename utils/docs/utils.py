import os
import subprocess

import frontmatter

REQUIRED_FIELDS = ["title", "description", "slug", "author", "date"]


def slug_from_path(file_path):
    rel = file_path.removeprefix("docs/").removesuffix(".mdx")
    return rel.replace("/", "-")


def parse_file(file_path, base_url):
    with open(file_path, "r", encoding="utf-8") as f:
        post = frontmatter.load(f)

    for field in REQUIRED_FIELDS:
        if field not in post.metadata or post.metadata.get(field) in [None, ""]:
            raise ValueError(f"Missing required field '{field}' in {file_path}")

    slug = post.metadata["slug"]
    expected_slug = slug_from_path(file_path)
    if slug != expected_slug:
        raise ValueError(
            f"Slug mismatch in {file_path}: got '{slug}', expected '{expected_slug}'"
        )

    doc = {
        "title": post.metadata["title"],
        "description": post.metadata["description"],
        "url": f"{base_url}/{file_path}",
        "slug": slug,
        "author": post.metadata["author"],
        "date": post.metadata["date"],
        "tags": post.metadata.get("tags", []),
    }

    source = post.metadata.get("source")
    if source:
        doc["source"] = source

    return doc


def collect_docs_recursive(dir_path, base_url, slugs, authors, tags):
    docs = []

    if not os.path.isdir(dir_path):
        return docs

    entries = os.listdir(dir_path)
    mdx_files = sorted({e[:-4] for e in entries if e.endswith(".mdx")})
    dirs = {e for e in entries if os.path.isdir(os.path.join(dir_path, e))}

    for name in mdx_files:
        file_path = os.path.join(dir_path, f"{name}.mdx")
        doc = parse_file(file_path, base_url)

        if doc["slug"] in slugs:
            raise ValueError(f"Duplicate slug: {doc['slug']}")
        slugs.add(doc["slug"])

        authors.add(doc["author"])
        tags.update(doc["tags"])

        if name in dirs:
            child_dir = os.path.join(dir_path, name)
            children = collect_docs_recursive(child_dir, base_url, slugs, authors, tags)
            if children:
                doc["children"] = children

        docs.append(doc)

    return docs


def current_branch():
    ci_ref = os.environ.get("CI_COMMIT_REF_NAME")
    if ci_ref:
        return ci_ref
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            text=True,
        ).strip()
    except Exception:
        return "master"


def current_project_url():
    ci_url = os.environ.get("CI_PROJECT_URL")
    if ci_url:
        return ci_url.rstrip("/")
    try:
        remote = subprocess.check_output(
            ["git", "remote", "get-url", "origin"],
            text=True,
        ).strip()
        remote = remote.rstrip("/")
        if remote.startswith("git@gitlab.com:"):
            slug = remote.removeprefix("git@gitlab.com:").removesuffix(".git")
            return f"https://gitlab.com/{slug}"
        if remote.startswith("https://gitlab.com/"):
            if remote.endswith(".git"):
                remote = remote[:-4]
            return remote
    except Exception:
        pass
    return "https://gitlab.com/bitspaceorg/ai/colossal-code"


def collect_docs():
    slugs = set()
    authors = set()
    tags = set()
    branch = current_branch()
    project_url = current_project_url()
    base_url = f"{project_url}/-/raw/{branch}"

    docs = collect_docs_recursive("docs", base_url, slugs, authors, tags)

    return docs, sorted(authors), sorted(tags)
