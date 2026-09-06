#!/bin/sh
# 配布物に同梱する LICENSES.md を組み立てる。リポジトリのルートで実行する。
#
#   sh docs/credits/build.sh
#
# cargo-about は第三者ライブラリのぶんだけを生成させ、sfd と pdq 自身の
# ライセンスはここで連結する。cargo-about に自分たちのぶんも任せると、
# ライセンス判定が前置きの長い pdq/LICENSE を素の BSD 3-Clause と見なさず、
# SPDX の雛形 (Copyright (c) <year> <owner>) に差し替えてしまう。
# それでは Meta の著作権表示も派生ファイルの一覧も消えてしまう。
#
# cargo-about の導入は次の形で行う。0.9.1 では cli が既定の feature ではなく、
# 付けないと警告だけ出してバイナリを入れずに終了コード 0 を返す。
#
#   cargo install cargo-about --locked --version 0.9.1 --features cli

set -eu

out=LICENSES.md
third_party=$(mktemp)
trap 'rm -f "$third_party"' EXIT

cargo about generate \
    --config docs/credits/about.toml \
    --output-file "$third_party" \
    docs/credits/about.hbs

{
    cat <<'HEADER'
# Licenses

This archive contains the `sfd` and `phashsum` executables. The licenses that
apply to them are reproduced in full below.

## sfd

HEADER
    printf '```\n'
    cat LICENSE
    printf '```\n\n'

    cat <<'HEADER'
## Bundled PDQ port

The `pdq` component is a Rust port of the PDQ reference implementation from
facebook/ThreatExchange. Its upstream notice follows.

HEADER
    printf '```\n'
    cat pdq/LICENSE
    printf '```\n\n'

    cat "$third_party"
} > "$out"

echo "$out を作りました ($(wc -l < "$out" | tr -d ' ') 行)"
