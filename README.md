# pico-w-http
ベアメタルRustでHTTPクライアントを実装したプロジェクトです

embassyの[example](https://github.com/embassy-rs/embassy/blob/main/examples/rp/src/bin/wifi_blinky.rs)を参考に実装しています

## 環境構築
* 必要なツールをCargoでインストール
```sh
cargo install elf2rf2-rs flip-link
```

## 実行方法
* Raspberry pi pico w のBOOTSELボタンを押したままPCと接続し、以下のコマンドを実行
```sh
cargo run --release
```
* TeraTermなどでUSBシリアル通信を表示
## 説明
