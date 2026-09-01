# SFD (Similar File Detector)

膨大にある画像・動画ファイルの中から類似のファイルを検出するためのツールです。

# 使い方

```sh
$ sfd -hash ./dir/ hash.json
```

上記コマンドを実行すると ./dir 以下の全ての画像・動画ファイルを再帰的に探索し、その知覚ハッシュ等を hash.json に出力する。

```sh
$ cat hash.json
{"path":"./dir/1.jpg","bytes":100,"sha256":"xxx","pdq":"xxx"}
{"path":"./dir/2.png","bytes":200,"sha256":"xxx","pdq":"xxx"}
{"path":"./dir/3.mp4","bytes":300,"sha256":"xxx","pdq":["xxx","xxx","xxx"]}
```

また、hash.json を元に類似ファイルを探すには以下を実行する。

```sh
$ sfd -find hash.json find.json
```

# メモ

- 知覚ハッシュの計算には [pdq](https://github.com/facebook/ThreatExchange/tree/main/pdq) を用いる
    - リファレンス実装の逐語移植を [pdq/](pdq) に置いた。公式の回帰テストベクタとビット単位で一致する
    - 単体で使える `phashsum` コマンドも同梱している
- 動画は動画を 4 分割して 1/4, 2/4, 3/4 地点のフレームに対してそれぞれ知覚ハッシュを計算する
- 画像や動画のデコードには ffmpeg を用いる
    - 中間ファイルは作らず、ffmpeg の出力を PAM (P7) 形式でパイプから受け取る
    - PAM はヘッダに寸法とチャンネル数を持つので ffprobe を別に呼ばずに済む
    - 2400 万画素の画像だと中間 png は 36MB になり、書き出しと読み直しだけで数百 ms を無駄にする
    - iPhone の HEIC はタイルが数百のストリームに見えるため、`-map` と `-vf` を使ってはいけない
- 知覚ハッシュの計算前に 512x512 へ縮小する (`pdq::preprocess::reference_downsample`)
    - リファレンス CLI と同じ前処理。公式の期待値と揃う
    - 縮小しないと 2400 万画素で 1 枚 363ms かかる。縮小すれば 3.2ms
- sha256 の計算には [sha2](https://crates.io/crates/sha2) crate を用いる
- hash.json は 1 行 1 JSON (JSONL) とする
    - エラーが起きても中断せず `{"path":..., "error":...}` の行として残す
    - 中断しても再開できるよう、既に書かれているパスは読み飛ばす
    - 強制終了で末尾行が途中まで書かれている可能性があるので、パースできない末尾行は捨てる

# 未定

`sfd -find` は以下を決めてから実装する。

- find.json の形式
- 一致とみなすハミング距離のしきい値 (PDQ 公式の推奨は 31/256)
- 動画同士の比較方法。3 ハッシュ × 3 ハッシュのうち 1 つでも近ければ一致とするか
- 画像と動画を比較するか (動画から切り出した静止画を検出したいなら必要)
- 回転・反転を同一視するか。`pdq` の dihedral を使えば実現できるが、保持するハッシュが 8 倍になる
- quality による足切りをするか (のっぺりした画像は偶然一致しやすい)
- 探索方法。数万件なら全ペア総当たりで足りる (256bit の popcount は速い)。数十万件を超えるなら BK-tree か MIH

# ライセンス

BSD 3-Clause License ([LICENSE](LICENSE))

`pdq/` は [facebook/ThreatExchange](https://github.com/facebook/ThreatExchange/tree/main/pdq) の PDQ リファレンス実装 (Copyright (c) Meta Platforms, Inc. and affiliates、BSD 3-Clause) を Rust に移植したものです。上流の著作権表示と、どのファイルが何の派生物かは [pdq/LICENSE](pdq/LICENSE) にあります。本プロジェクトは Meta Platforms, Inc. とは無関係です。
