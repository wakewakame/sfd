# SFD (Similar File Detector)

Finds similar files among large collections of images and videos.

Decoding is done by [ffmpeg](https://ffmpeg.org/) and ffprobe, so install those first.

# Usage

## sfd hash

```sh
$ sfd hash ./dir/ hash.json
```

Walks `./dir` recursively and writes the perceptual hash of every image and video to `hash.json`.

If the run is interrupted, running the same command again continues where it left off.

`hash.json` holds one JSON object per line (JSONL), with the first line as a header.

```sh
$ cat hash.json
{"v":1,"root":"./dir"}
{"path":"1.jpg","bytes":100,"mtime":"2026-09-02T01:23:45Z","sha256":"xxx","width":4032,"height":3024,"pdq":[{"t":0.0,"hash":"xxx","dihedral":["xxx",...],"quality":100}]}
{"path":"3.mp4","bytes":300,"mtime":"2026-07-01T09:30:00Z","sha256":"xxx","width":1920,"height":1080,"duration":123.4,"pdq":[{"t":30.8,"hash":"xxx","dihedral":[...],"quality":100},{"t":61.7,...},{"t":92.5,...}]}
{"path":"4.jpg","bytes":50,"mtime":"2026-06-15T18:00:00Z","sha256":"xxx","error":"ffmpeg failed: ..."}
{"path":"5-locked","mtime":"2026-05-01T00:00:00Z","error":"cannot read the directory: Permission denied (os error 13)"}
```

| Field | Meaning |
| --- | --- |
| `path` | relative to `root` in the header |
| `pdq` | perceptual hashes. An image is stored as a one-frame video, so the shape never changes |
| `t` | position of the frame within a video, in seconds. Always 0 for images |
| `dihedral` | the same frame rotated and flipped, 7 hashes in total |
| `duration` | length of a video in seconds. Its presence is what distinguishes videos from images |
| `quality` | 0..=100. Lower for flat, featureless images |
| `error` | the file could not be processed. Unreadable directories are recorded here too |

When reading the file:

- if the same `path` appears more than once, **the last line wins**
- entries that could not be read may be missing `bytes` or `sha256`

Passing `--no-dihedral` makes `hash.json` smaller but gives up finding rotated copies.
**Those hashes can only be produced while hashing**, so wanting them later means
rescanning every file.

See `sfd hash --help` for the remaining options.

## sfd find

```sh
$ sfd find hash.json find.json
```

Reads `hash.json` and writes groups of similar files to `find.json`.

Starting from the first file, everything within the threshold is listed under it.
A file that has already appeared in one group never appears in another. Files with
no similar counterpart are omitted.

```sh
$ cat find.json
{"v":1,"threshold":31}
{"path":"1.jpg","bytes":100,"width":4032,"height":3024,"similar":[{"path":"1-copy.jpg","distance":0,"identical":true,"bytes":100,"width":4032,"height":3024},{"path":"1-rotated.jpg","distance":18,"transform":"rotate270","bytes":90,"width":3024,"height":4032}]}
```

| Field | Meaning |
| --- | --- |
| `path` | head of the group. Ordered by size then resolution, so it tends to be the copy worth keeping |
| `distance` | distance from the head (0..=256). Smaller means more similar |
| `identical` | the sha256 matches the head. One of them can be deleted without looking |
| `transform` | set when the match only holds after rotating or flipping |

How the comparison works:

- **images and videos are never compared** with each other (told apart by `duration`)
- **for videos, all three sampled positions must match** pairwise. Accepting a single
  match would produce false positives, since black and white frames are common
- `--threshold` changes the cutoff and `--min-quality` skips low-quality entries

# License

BSD 3-Clause License ([LICENSE](LICENSE))

`pdq/` is a Rust port of the PDQ reference implementation from
[facebook/ThreatExchange](https://github.com/facebook/ThreatExchange/tree/main/pdq)
(Copyright (c) Meta Platforms, Inc. and affiliates, BSD 3-Clause). The upstream
notice, and which files are derived from what, are in [pdq/LICENSE](pdq/LICENSE).
This project is not affiliated with Meta Platforms, Inc.
