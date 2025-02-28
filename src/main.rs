
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
use reqwless::client::HttpClient;
use reqwless::request::Method;

// logger
use defmt::*;
use {defmt_rtt as _, panic_probe as _};

// WiFiのSSIDとパスワード
const WIFI_SSID: &str = "SSID";
const WIFI_PASSWORD: &str = "PASSWORD";

// 接続先URL（HTTP限定）
const TEST_URL: &str = "http://example.com";

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
    
    // 乱数生成器を初期化
    let mut rng = RoscRng;

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
    unwrap!(spawner.spawn(cyw43_task(runner)));

    // WiFiチップの初期化
    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;


    let config = Config::dhcpv4(Default::default());
    let seed = rng.next_u64();
    
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(net_device, config, RESOURCES.init(StackResources::new()), seed);
    // ネットワークスタックのタスクを起動
    unwrap!(spawner.spawn(net_task(runner)));

    // WiFiに接続
    loop {
        // 接続に成功するまで繰り返す
        match control.join(WIFI_SSID, JoinOptions::new(WIFI_PASSWORD.as_bytes())).await {
            Ok(_) => break,
            Err(err) => {
                log::info!("join failed with status={}", err.status);
            }
        }
    }

    log::info!("waiting for DHCP ...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }

    log::info!("DHCP is up!");

    log::info!("waiting for link up ...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }

    log::info!("link is up!");

    log::info!("waiting for stack to be up ...");
    stack.wait_config_up().await;
    log::info!("stack is up!");

    loop {
        let mut rx_buffer = [0; 8192];
        
        let client_state = TcpClientState::<1, 1024, 1024>::new();
        let tcp_client = TcpClient::new(stack, &client_state);
        let dns_client = DnsSocket::new(stack);

        let mut http_client = HttpClient::new(&tcp_client, &dns_client);

        log::info!("connecting to {}", TEST_URL);

        let mut request = match http_client.request(Method::GET, &TEST_URL).await {
            Ok(req) => req,
            Err(e) => {
                log::error!("Failed to make HTTP request: {:?}", e);
                return;
            }
        };

        log::info!("sending request ...");

        let response = match request.send(&mut rx_buffer).await {
            Ok(resp) => resp,
            Err(_e) => {
                log::error!("Failed to send HTTP request");
                return;
            }
        };

        log::info!("response status: {:?}", response.status);

        let body = match from_utf8(response.body().read_to_end().await.unwrap()) {
            Ok(b) => b,
            Err(_e) => {
                log::error!("Failed to read response body");
                return;
            }
        };
        
        log::info!("response body: {}", body);

        Timer::after(Duration::from_secs(10)).await;
    }
}