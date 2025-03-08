# pico-w-http
ベアメタルRustでHTTPクライアントを実装したプロジェクトです

embassyの[example](https://github.com/embassy-rs/embassy/blob/main/examples/rp/src/bin/wifi_blinky.rs)を参考に実装しています

## 環境構築
* 必要なツールをCargoでインストール
```sh
cargo install elf2uf2-rs flip-link
```

## 実行方法
* src/main.rsのSSIDとPASSWORDを接続するWiFiのものへ変更
* src/main.rsのURLを接続したいURLへ変更
* Raspberry pi pico w のBOOTSELボタンを押したままPCと接続し、以下のコマンドを実行
```sh
cargo run --release
```
* TeraTermなどでUSBシリアル通信を表示
## 説明
修正中の不具合
* 一部サイトへアクセスすると，TLSのエラーでレスポンスが取得できない
* 一部サイトへアクセスすると，ステータスコードは200だが，レスポンスボディが表示されない
* レスポンスボディが大きなサイトは，結果の出力が途中で切れる
