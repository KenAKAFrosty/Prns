# prnsd source bundle staging

Release automation writes the exact commit-bound `source.zip` and
`source.zip.sha256` here before invoking the root Dockerfile. The generated
files are ignored; this directory exists so ordinary local image builds remain
valid even when no release source bundle has been staged.

To exercise the source-enabled image path locally:

```sh
./tools/prns release source package -- --output release/source-bundle/source.zip
docker build .
```
