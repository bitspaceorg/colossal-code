#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

python utils/docs/validate.py
python utils/docs/build_index.py
git add docs/data.json
