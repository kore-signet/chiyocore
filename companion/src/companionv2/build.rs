use alloc::{borrow::Cow, sync::Arc};
use chiyo_hal::{EspMutex, EspRwLock};
use chiyocore::{
    builder::BuildChiyocoreLayer,
    meshcore,
    simple_mesh::{
        SimpleMesh,
        storage::channel::{Channel, ChannelStorage},
    },
};
use meshcore::crypto::ChannelKeys;
use serde::{Deserialize, Serialize};

use crate::{companion_protocol::protocol::ChannelCompanionSink, companionv2::Companion};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompanionConfig {
    pub id: Cow<'static, str>,
    pub tcp_port: u16,
}

impl BuildChiyocoreLayer for Companion {
    type Input = (&'static str, CompanionConfig);
    type Output = Arc<EspMutex<Companion>>;

    async fn build<T: 'static>(
        spawner: &embassy_executor::Spawner,
        chiyocore: &chiyocore::builder::Chiyocore<T, chiyocore::builder::ChiyocoreSetupData>,
        mesh: &alloc::sync::Arc<EspRwLock<SimpleMesh>>,
        config: &Self::Input,
    ) -> Self::Output {
        let (_, config) = config;
        
        add_channels(&mut *chiyocore.mesh_storage().channels.write().await).await;

        let (tcp_tx, tcp_rx) = thingbuf::mpsc::channel(16);
        let slot = &config.id;
        // let (tcp_tx, tcp_rx) = crate::companion_protocol::tcp::TCP_COMPANION_CHANNEL.split();

        let companion = Arc::new(EspMutex::new(
            Companion::new(
                slot,
                chiyocore.rtc(),
                chiyocore.mesh_storage().clone(),
                chiyocore.config_db(),
                chiyocore.main_fs(),
                chiyocore.log_fs(),
                mesh,
                ChannelCompanionSink::new(tcp_tx),
            )
            .await
            .unwrap(),
        ));

        spawner.spawn(
            crate::companion_protocol::tcp::tcp_companion(
                chiyocore
                    .setup_data
                    .net_stack
                    .expect("companion layer requires network to be setup!"),
                tcp_rx,
                Arc::clone(&companion),
                config.tcp_port,
            )
            .unwrap(),
        );

        companion
    }
}

async fn add_channels(channels: &mut ChannelStorage) {
    channels
        .insert(Channel::from_keys(0, "public", ChannelKeys::public()))
        .await
        .unwrap();
    channels
        .insert(Channel::from_keys(
            1,
            "#test",
            ChannelKeys::from_hashtag("#test"),
        ))
        .await
        .unwrap();
    channels
        .insert(Channel::from_keys(
            2,
            "#emitestcorner",
            ChannelKeys::from_hashtag("#emitestcorner"),
        ))
        .await
        .unwrap();
}
