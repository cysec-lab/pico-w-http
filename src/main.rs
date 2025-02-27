#![no_std]
#![no_main]
use core::str::from_utf8;

use cyw43::JoinOptions;
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use embassy_rp::usb::{Driver, InterruptHandler as usb_InterruptHandler};

use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Config, StackResources};
use embassy_rp::clocks::RoscRng;
use rand::RngCore;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::Method;
use serde::Deserialize;

use {defmt_rtt as _, panic_probe as _, serde_json_core};

const WIFI_SSID: &str = "SSID";
const WIFI_PASSWORD: &str = "PASSWORD";

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    USBCTRL_IRQ => usb_InterruptHandler<USB>;
});

#[embassy_executor::task]
async fn cyw43_task(runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // ペリフェラルを初期化
    let p = embassy_rp::init(Default::default());

    let driver = Driver::new(p.USB, Irqs);

    let mut rng = RoscRng;

    spawner.spawn(logger_task(driver)).unwrap();
    let mut counter = 0;

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

    // WiFiチップの初期化
    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    // WiFiチップのタスクを起動
    unwrap!(spawner.spawn(cyw43_task(runner)));


    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let delay = Duration::from_secs(1);

    let config = Config::dhcpv4(Default::default());

    let seed = rng.next_u64();
    
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(net_device, config, RESOURCES.init(StackResources::new()), seed);

    unwrap!(spawner.spawn(net_task(runner)));

    loop {
        match control.join(WIFI_SSID, JoinOptions::new(WIFI_PASSWORD.as_bytes())).await {
            Ok(_) => break,
            Err(err) => {
                log::info!("join failed with status={}", err.status);
            }
        }
    }

    log::info!("waiting for HDCP ...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }

    log::info!("HDCP is up!");

    log::info!("waiting for link up ...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }

    log::info!("link is up!");

    log::info!("waiting for stack to be up ...");
    stack.wait_config_up().await;
    log::info!("stack is up!");


}