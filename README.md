# SFD (Similar File Detector)

膨大にある画像・動画ファイルの中から類似のファイルを検出するためのツールです。

デコードに [ffmpeg](https://ffmpeg.org/) と ffprobe を使うので、あらかじめインストールしておいてください。

# 使い方

## sfd hash

```sh
$ sfd hash ./dir/ hash.json
```

./dir 以下の全ての画像・動画ファイルを再帰的に探索し、その知覚ハッシュ等を hash.json に出力する。

中断しても、同じコマンドをもう一度実行すれば続きから再開する。

hash.json は 1 行 1 JSON (JSONL) で、1 行目をヘッダとする。

```sh
$ cat hash.json
{"v":1,"root":"./dir"}
{"path":"1.jpg","bytes":100,"mtime":"2026-09-02T01:23:45Z","sha256":"xxx","width":4032,"height":3024,"pdq":[{"t":0.0,"hash":"xxx","dihedral":["xxx",...],"quality":100}]}
{"path":"3.mp4","bytes":300,"mtime":"2026-07-01T09:30:00Z","sha256":"xxx","width":1920,"height":1080,"duration":123.4,"pdq":[{"t":30.8,...},{"t":61.7,...},{"t":92.5,...}]}
{"path":"4.jpg","bytes":50,"mtime":"2026-06-15T18:00:00Z","sha256":"xxx","error":"ffmpeg が失敗しました: ..."}
```

| フィールド | 内容 |
| --- | --- |
| `path` | ヘッダの `root` からの相対パス |
| `pdq` | 知覚ハッシュ。画像も「1 フレームの動画」として同じ形で持つ |
| `t` | 動画のどの位置のフレームか (秒)。画像は 0 |
| `dihedral` | 回転・反転させた 7 通りのハッシュ |
| `duration` | 動画の尺 (秒)。これを持つかどうかが画像と動画の区別になる |
| `quality` | 0..=100。のっぺりした画像ほど低い |
| `error` | 処理に失敗したもの。読めなかったディレクトリもここに残る |

読み取るときの注意:

- 同じ `path` の行が複数あれば、**後の行が有効**
- 読めなかったものは `bytes` や `sha256` を持たないことがある

`--no-dihedral` を付けると hash.json は小さくなるが、回転したコピーを検出できなくなる。**これは知覚ハッシュの計算時にしか作れない**ので、後から必要になると全ファイルの再スキャンになる。

その他のオプションは `sfd hash --help` を参照。

## sfd find

```sh
$ sfd find hash.json find.json
```

hash.json を元に、似ているファイルをまとめて find.json に出力する。

先頭のファイルから順に、しきい値以内にある他のファイルを列挙する。一度どこかのグループに現れたファイルは、以降のグループには現れない。似たファイルが 1 つもないものは出力しない。

```sh
$ cat find.json
{"v":1,"threshold":31}
{"path":"1.jpg","bytes":100,"width":4032,"height":3024,"similar":[{"path":"1-copy.jpg","distance":0,"identical":true,"bytes":100,"width":4032,"height":3024},{"path":"1-rotated.jpg","distance":18,"transform":"rotate270","bytes":90,"width":3024,"height":4032}]}
```

| フィールド | 内容 |
| --- | --- |
| `path` | グループの先頭。バイト数・解像度の大きいものが来るので、残す候補になりやすい |
| `distance` | 先頭との距離 (0..=256)。小さいほど似ている |
| `identical` | 先頭と sha256 が一致する。見た目を比べるまでもなく片方を消してよい |
| `transform` | 回転・反転させた状態で一致した場合の変換名 |

画像と動画は比較しない。動画は 3 箇所すべてが一致することを要求する。

しきい値は `--threshold` で、品質による足切りは `--min-quality` で変えられる。詳しくは `sfd find --help` を参照。

# ライセンス

BSD 3-Clause License ([LICENSE](LICENSE))

`pdq/` は [facebook/ThreatExchange](https://github.com/facebook/ThreatExchange/tree/main/pdq) の PDQ リファレンス実装 (Copyright (c) Meta Platforms, Inc. and affiliates、BSD 3-Clause) を Rust に移植したものです。上流の著作権表示と、どのファイルが何の派生物かは [pdq/LICENSE](pdq/LICENSE) にあります。本プロジェクトは Meta Platforms, Inc. とは無関係です。
