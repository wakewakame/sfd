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
- 動画は動画を 4 分割して 1/4, 2/4, 3/4 地点のフレームに対してそれぞれ知覚ハッシュを計算する
- 画像や動画は全て一旦 png に変換する
- 画像や動画のデコードには ffmpeg を用いる

# ライセンス

BSD 3-Clause License ([LICENSE](LICENSE))

`pdq/` は [facebook/ThreatExchange](https://github.com/facebook/ThreatExchange/tree/main/pdq) の PDQ リファレンス実装 (Copyright (c) Meta Platforms, Inc. and affiliates、BSD 3-Clause) を Rust に移植したものです。上流の著作権表示と、どのファイルが何の派生物かは [pdq/LICENSE](pdq/LICENSE) にあります。本プロジェクトは Meta Platforms, Inc. とは無関係です。
