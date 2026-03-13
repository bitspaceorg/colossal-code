import os
import re

from utils import collect_docs


def collect_all_slugs(docs):
    slugs = set()
    for doc in docs:
        slugs.add(doc["slug"])
        if "children" in doc:
            slugs.update(collect_all_slugs(doc["children"]))
    return slugs


def validate_links(docs_dir, valid_slugs):
    link_re = re.compile(r"\[([^\]]*)\]\(([^)]+)\)")
    errors = []

    for root, _dirs, files in os.walk(docs_dir):
        for fname in files:
            if not fname.endswith(".mdx"):
                continue
            fpath = os.path.join(root, fname)
            with open(fpath, "r", encoding="utf-8") as f:
                content = f.read()

            for match in link_re.finditer(content):
                target = match.group(2)
                if target.startswith("http") or target.startswith("#") or "." in target:
                    continue

                slug_part = target.split("#")[0]
                if slug_part and slug_part not in valid_slugs:
                    errors.append(
                        f"  {fpath}: [{match.group(1)}]({target}) -> unknown slug '{slug_part}'"
                    )

    return errors


def main():
    docs, _authors, _tags = collect_docs()
    valid_slugs = collect_all_slugs(docs)

    errors = validate_links("docs", valid_slugs)
    if errors:
        print(f"Found {len(errors)} broken slug references:")
        for error in errors:
            print(error)
        raise SystemExit(1)

    print("[=== all mdx files are valid ===]")


if __name__ == "__main__":
    main()
