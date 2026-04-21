
#![no_std]
#![no_main]

        extern crate alloc;

        use alloc::sync::Arc;
        use chiyocore::meshcore;
        use chiyocore::builder::{Chiyocore, ChiyocorePeripherals, ChiyocoreSetupData};
        use chiyocore::ping_bot::PingBot;
        use chiyocore_companion::companionv2::{Companion};
        use embassy_executor::Spawner;

        use chiyocore::simple_mesh::storage::channel::Channel;
        use meshcore::crypto::ChannelKeys;
        
        use chiyo_hal::{embassy_time, esp_rtos, storage::*};
        
        use chiyo_hal::esp_println as _;
        use chiyo_hal::esp_backtrace as _;
        use esp_hal::clock::CpuClock;
        use esp_hal::rng::Trng;
        use esp_hal::system::Stack;
        use esp_hal::timer::timg::TimerGroup;
        use esp_rtos::embassy::Executor;
        use litemap::LiteMap;
        // use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
        use chiyo_hal::EspMutex;
        use chiyocore::builder::{ChiyocoreNode, BuildChiyocoreLayer, BuildChiyocoreSet};
        use chiyocore::simple_mesh::MeshLayerGet;
        use meshcore::identity::LocalIdentity;
        use smol_str::SmolStr;
        use chiyocore::static_cell::StaticCell;
        use core::ffi::CStr;
        use alloc::borrow::Cow;