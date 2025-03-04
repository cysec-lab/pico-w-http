
#![no_std]
#![no_main]

// バイト列をutf8へ変換
use core::str::from_utf8;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::usb::{InterruptHandler as UsbInterruptHandler, Driver};
use embassy_rp::clocks::RoscRng;

use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Config, StackResources};

use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use rand::RngCore;

use cyw43::JoinOptions;
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};

// 組み込み向けHTTPクライアント
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::Method;

// logger
use panic_halt as _;

// WiFiのSSIDとパスワード
const WIFI_SSID: &str = "SSID";
const WIFI_PASSWORD: &str = "PASS";

// 接続先URL(HTTP/HTTPS)
const TEST_URL: &str = "https://example.com";

// 割り込みと割り込みハンドラを対応付け
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

// CYW43のドライバを実行するタスク
#[embassy_executor::task]
async fn cyw43_task(runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>) -> ! {
    runner.run().await
}

// ネットワークスタックのタスク
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

// USBでのログ出力タスク
#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

// エントリーポイント
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // ペリフェラルを初期化
    let p = embassy_rp::init(Default::default());

    // USBドライバを初期化, USBでログを出力するタスクの起動
    let driver = Driver::new(p.USB, Irqs);
    spawner.spawn(logger_task(driver)).unwrap();

    // WiFi ファームウェアとCLMを読み込む
    let fw = include_bytes!("../firmware/43439A0.bin");
    let clm = include_bytes!("../firmware/43439A0_clm.bin");

    // GPIO23：WiFiチップの電源制御用ピン
    let pwr = Output::new(p.PIN_23, Level::Low);
    // GPIO25：WiFiチップのSPI通信用CS（チップセレクト）ピン
    let cs = Output::new(p.PIN_25, Level::High);
    // PIO0：WiFiチップのSPI通信用PIO
    let mut pio = Pio::new(p.PIO0, Irqs);
    // PioSpi：WiFiチップのSPI通信用インターフェース
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    // WiFiチップのドライバを生成
    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    // WiFiチップのタスクを起動
    spawner.spawn(cyw43_task(runner)).unwrap();

    // WiFiチップの初期化
    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    // DHCPv4の設定
    let config = Config::dhcpv4(Default::default());

    // 乱数生成器を初期化
    let mut rng = RoscRng;
    let seed = rng.next_u64();
    
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(net_device, config, RESOURCES.init(StackResources::new()), seed);
    
    // ネットワークスタックのタスクを起動
    spawner.spawn(net_task(runner)).unwrap();

    log::info!("Connecting to WiFi ...");
    // WiFiに接続
    loop {
        match control.join(WIFI_SSID, JoinOptions::new(WIFI_PASSWORD.as_bytes())).await {
            Ok(_) => break,
            Err(err) => {
                log::info!("join failed with status={}", err.status);
            }
        }
    }

    // DHCPの完了を待つ
    log::info!("waiting for DHCP ...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    log::info!("DHCP is up!");

    // リンクアップの完了を待つ
    log::info!("waiting for link up ...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }
    log::info!("link is up!");

    // ネットワークスタックの設定が完了するまで待つ
    log::info!("waiting for stack to be up ...");
    stack.wait_config_up().await;
    log::info!("stack is up!");


    // HTTPクライアント
    loop {
        // HTTPレスポンスのバッファ
        let mut rx_buffer = [0; 8192];
        // TLSのバッファ
        let mut tls_read_buffer = [0; 16640];
        let mut tls_write_buffer = [0; 16640];
        

        let client_state = TcpClientState::<1, 1024, 1024>::new();
        // TCPクライアントとDNSクライアントを生成
        let tcp_client = TcpClient::new(stack, &client_state);
        let dns_client = DnsSocket::new(stack);
        // TLS設定(HTPPSの場合)
        let tls_config = TlsConfig::new(seed, &mut tls_read_buffer, &mut tls_write_buffer, TlsVerify::None);

        // httpsならnew_with_tls, httpならnew
        let mut http_client = HttpClient::new_with_tls(&tcp_client, &dns_client, tls_config);

        log::info!("connecting to {}", TEST_URL);
        // HTTPリクエストを作製
        let mut request = match http_client.request(Method::GET, &TEST_URL).await {
            Ok(req) => req,
            Err(e) => {
                log::error!("Failed to make HTTP request: {:?}", e);
                return;
            }
        };

        log::info!("sending request ...");
        // HTTPリクエストを送信
        let response = match request.send(&mut rx_buffer).await {
            Ok(resp) => resp,
            Err(_e) => {
                log::error!("Failed to send HTTP request");
                return;
            }
        };

        // ステータスコードの表示
        log::info!("response status: {:?}", response.status);

        // レスポンスボディの表示
        let body = match from_utf8(response.body().read_to_end().await.unwrap()) {
            Ok(b) => b,
            Err(_e) => {
                log::error!("Failed to read response body");
                return;
            }
        };
        log::info!("response body: {:?}", &body);

        // 10秒毎に繰り返す
        Timer::after(Duration::from_secs(10)).await;
    }
}