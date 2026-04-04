#![no_std]
#![feature(allocator_api)]
extern crate alloc;
use core::{fmt::Write as _, net::SocketAddr};

use alloc::{sync::Arc, vec::Vec};
use chiyocore::{
    CompanionResult, EspMutex, PacketStatus,
    builder::BuildChiyocoreLayer,
    meshcore, mk_static,
    simple_mesh::{SimpleMesh, SimpleMeshLayer},
};
use edge_http::Method;
use edge_nal_embassy::TcpBuffers;
use embassy_executor::SendSpawner;
use embassy_net::Stack;
use embassy_sync::{
    channel::{Channel, Receiver, Sender},
    rwlock::RwLock,
    signal::Signal,
};
use embassy_time::Duration;
use embedded_io_async::Read as _;
use esp_hal::{rng::Trng, rtc_cntl::Rtc};
use mbedtls_rs::{ClientSessionConfig, Tls, TlsConnector, nal::TcpConnect};
// use mbedtls_rs::Tls;
use meshcore::{
    Path,
    identity::ForeignIdentity,
    payloads::{Advert, AdvertisementExtraData, AppdataFlags, TextMessageData},
};
// use reqwless::{client::{HttpClient, TlsConfig}, request::RequestBuilder};
use serde::{Deserialize, Deserializer, Serialize};
use smol_str::SmolStr;

type EspSignal<T> = Signal<esp_sync::RawMutex, T>;

pub struct TTCBot {
    pub channels: [SmolStr; 2],
    pub rtc: Arc<Rtc<'static>>,
    pub req: Sender<'static, esp_sync::RawMutex, (u16, Arc<EspSignal<heapless::String<328>>>), 16>,
    pub mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
}

impl SimpleMeshLayer for TTCBot {
    async fn packet<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_bytes: &'f [u8],
        _packet_status: PacketStatus,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn text_message<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        contact: &'f chiyocore::simple_mesh::storage::contact::CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> CompanionResult<()> {
        let msg = message.as_utf8()?;
        let Ok(msg) = msg.trim_end_matches('\0').trim().parse::<u16>() else {
            return Ok(());
        };

        let signal = Arc::new(EspSignal::new());
        self.req.send((msg, Arc::clone(&signal))).await;

        SendSpawner::for_current_executor().await.spawn(handle_req(
            Arc::clone(&self.mesh),
            signal,
            contact.as_identity(),
            contact.path.clone(),
            Arc::clone(&self.rtc),
        ));
        // s
        // self.req.send(msg).await;

        Ok(())
    }

    async fn group_text<'f>(
        &'f mut self,
        mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        channel: &'f chiyocore::simple_mesh::storage::channel::Channel,
        message: &'f meshcore::payloads::TextMessageData<'_>,
    ) -> CompanionResult<()> {
        // log::info!("nya: {}", message.as_utf8().unwrap());
        if !self.channels.contains(&channel.name) {
            return Ok(());
        }

        let msg = message.as_utf8()?.trim_end_matches('\0').trim();
        let Some((_user, msg)) = msg.split_once(':') else {
            return Ok(());
        };

        log::info!("{}", msg.trim());

        if msg.trim() == "!cafe_advert" {
            let message = "cafe/xiaos3: sending advert!";
            let message = TextMessageData::plaintext(
                (self.rtc.current_time_us() / 1_000_000) as u32,
                message.as_bytes(),
            );
            let delay = chiyocore::timing::rx_retransmit_delay(packet);
            mesh.send_channel_message(&channel.as_keys(), &message, Some(delay))
                .await?;

            // todo: standardize this better
            let appdata = AdvertisementExtraData {
                flags: AppdataFlags::HAS_NAME | AppdataFlags::IS_CHAT_NODE,
                latitude: None,
                longitude: None,
                feature_1: None,
                feature_2: None,
                name: Some(b"cafe/xiaos3".into()),
            };

            let random_bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
            // let mesh = self.mesh.read().await;
            let advert = mesh.identity.make_advert(
                (self.rtc.current_time_us() / 1_000_000) as u32,
                appdata,
                random_bytes,
            );

            mesh.send_flood_packet::<Advert>(&advert, None).await?;
        }

        Ok(())
    }

    async fn ack<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _ack: &'f meshcore::payloads::Ack,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn advert<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _advert: &'f meshcore::payloads::Advert<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn returned_path<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f chiyocore::simple_mesh::storage::contact::CachedContact,
        _path: &'f meshcore::payloads::ReturnedPath<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn response<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f chiyocore::simple_mesh::storage::contact::CachedContact,
        _response: &'f [u8],
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn request<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f chiyocore::simple_mesh::storage::contact::CachedContact,
        _request: &'f meshcore::payloads::RequestPayload<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn anonymous_request<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f meshcore::identity::ForeignIdentity,
        _data: &'f [u8],
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn trace_packet<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _snrs: &'f [i8],
        _trace: &'f meshcore::payloads::TracePacket<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn control_packet<'f>(
        &'f mut self,
        _mesh: &'f chiyocore::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        _payload: &'f meshcore::payloads::ControlPayload,
    ) -> CompanionResult<()> {
        Ok(())
    }
}

impl BuildChiyocoreLayer for TTCBot {
    type Input = ();
    type Output = Arc<EspMutex<TTCBot>>;

    fn build<T: 'static>(
        spawner: &embassy_executor::Spawner,
        chiyocore: &chiyocore::builder::Chiyocore<T, chiyocore::builder::ChiyocoreSetupData>,
        mesh: &Arc<
            embassy_sync::rwlock::RwLock<esp_sync::RawMutex, chiyocore::simple_mesh::SimpleMesh>,
        >,
        _cfg: &Self::Input,
    ) -> impl Future<Output = Self::Output> {
        let (req_tx, req_rx) = (TTC_REQ_CHANNEL.sender(), TTC_REQ_CHANNEL.receiver());

        spawner.must_spawn(ttc_http_task(
            chiyocore.setup_data.net_stack.unwrap(),
            req_rx,
        ));

        core::future::ready(Arc::new(EspMutex::new(TTCBot {
            channels: ["#test".into(), "#emitestcorner".into()],
            rtc: Arc::clone(chiyocore.rtc()),
            req: req_tx,
            mesh: Arc::clone(mesh),
        })))
    }
}

static TTC_REQ_CHANNEL: Channel<
    esp_sync::RawMutex,
    (u16, Arc<EspSignal<heapless::String<328>>>),
    16,
> = Channel::new();

// use mbedtls_rs::sys::hook::backend::embassy::
// use mbedtls_rs::sys::hook::backend::embassy::

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TTCRes<'a> {
    #[serde(borrow)]
    predictions: heapless::Vec<TTCPrediction<'a>, 8>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TTCPrediction<'a> {
    route_tag: &'a str,
    stop_tag: &'a str,
    route_title: &'a str,
    stop_title: &'a str,
    #[serde(default)]
    direction: Option<TTCDirection<'a>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TTCDirection<'a> {
    title: &'a str,
    #[serde(default, deserialize_with = "deser_ttc_pred")]
    prediction: heapless::Vec<TTCDirectionalPrediction<'a>, 8>,
}

fn deser_ttc_pred<'de, D>(
    deser: D,
) -> Result<heapless::Vec<TTCDirectionalPrediction<'de>, 8>, D::Error>
where
    D: Deserializer<'de>,
{
    OneOrMany::deserialize(deser).map(|v| v.into())
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// Single value
    One(T),
    /// Array of values
    Vec(heapless::Vec<T, 8>),
}

impl<T> From<OneOrMany<T>> for heapless::Vec<T, 8> {
    fn from(from: OneOrMany<T>) -> Self {
        match from {
            OneOrMany::One(val) => heapless::Vec::from_iter([val]),
            OneOrMany::Vec(vec) => vec,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TTCDirectionalPrediction<'a> {
    seconds: &'a str,
    minutes: &'a str,
    trip_tag: &'a str,
    vehicle: &'a str,
}

#[embassy_executor::task(pool_size = 8)]
async fn handle_req(
    mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
    res_sig: Arc<EspSignal<heapless::String<328>>>,
    contact: ForeignIdentity,
    contact_path: Option<Path<'static>>,
    rtc: Arc<Rtc<'static>>,
) {
    let Ok(res_full) = embassy_time::with_timeout(Duration::from_secs(15), res_sig.wait()).await
    else {
        return;
    };

    let res = res_full.trim_end();

    // let base_delay = chiyocore::timing::rx_retransmit_delay(packet);
    // let mesh = mesh.read().await;

    if res.len() > 120 {
        for line in res.split_terminator('\n') {
            let text = TextMessageData::plaintext(
                (rtc.current_time_us() / 1_000_000) as u32,
                line.as_bytes(),
            );
            SimpleMesh::send_to_contact_with_retry::<TextMessageData<'_>>(
                &mesh,
                &contact,
                contact_path.clone(),
                &text,
            )
            .await
            .unwrap()
            .wait()
            .await;
            // mesh.send_direct_message(&contact, contact_path.clone(), text, None)
            // .await;
            embassy_time::Timer::after_millis(500).await;
        }
    } else {
        let text =
            TextMessageData::plaintext((rtc.current_time_us() / 1_000_000) as u32, res.as_bytes());
        SimpleMesh::send_to_contact_with_retry::<TextMessageData<'_>>(
            &mesh,
            &contact,
            contact_path.clone(),
            &text,
        )
        .await
        .unwrap()
        .wait()
        .await;

        // mesh.send_direct_message(&contact, contact_path.clone(), text, None)
        // .await;/
        // mesh.send_to_contact(contact, path, message)
    }
    // res.splitn(n, pat)
}

// https://webservices.umoiq.com/service/publicJSONFeed?command=predictions&a=ttc&stopId=5900
#[embassy_executor::task]
async fn ttc_http_task(
    stack: Stack<'static>,
    rx: Receiver<'static, esp_sync::RawMutex, (u16, Arc<EspSignal<heapless::String<328>>>), 16>,
) {
    let _accel = EspAccelQueueCustom::start();

    let mut res_buf = Vec::new_in(esp_alloc::ExternalMemory);
    let mut normal_buf = Vec::new_in(esp_alloc::ExternalMemory);
    let mut complete_buf = Vec::new_in(esp_alloc::ExternalMemory);

    res_buf.resize(8192, 0u8);
    normal_buf.resize(8192, 0u8);
    complete_buf.reserve(8192);

    let mut rng = Trng::try_new().unwrap();
    let tls = Tls::new(&mut rng).unwrap();

    // let buf =
    let tcp_buffers = mk_static!(TcpBuffers<2, 1024, 1024>, TcpBuffers::new());
    let tcp = edge_nal_embassy::Tcp::new(stack, tcp_buffers);
    let _dns = edge_nal_embassy::Dns::new(stack);

    let server_ip = stack
        .dns_query("webservices.umoiq.com", embassy_net::dns::DnsQueryType::A)
        .await
        .unwrap();
    let tls_conn = TlsConnector::new(
        tls.reference(),
        tcp,
        &ClientSessionConfig {
            ca_chain: None,
            creds: None,
            server_name: Some(c"webservices.umoiq.com"),
            auth_mode: mbedtls_rs::AuthMode::None,
            min_version: mbedtls_rs::TlsVersion::Tls1_2,
        },
    );

    let mut conn = edge_http::io::client::Connection::<_, 32>::new(
        &mut normal_buf,
        &tls_conn,
        SocketAddr::new(server_ip[0].into(), 443),
    );

    async fn make_req<'a>(
        complete_buf: &'a mut Vec<u8, esp_alloc::ExternalMemory>,
        res_buf: &'a mut [u8],
        _escape_buf: &'a mut [u8],
        conn: &mut edge_http::io::client::Connection<'_, impl TcpConnect, 32>,
        path: &str,
    ) -> Option<TTCRes<'a>> {
        complete_buf.clear();

        conn.initiate_request(
            true,
            Method::Get,
            path,
            &[("Host", "webservices.umoiq.com")],
        )
        .await
        .ok()?;

        conn.initiate_response().await.ok()?;

        loop {
            let Ok(len) = conn.read(res_buf).await else {
                return None;
            };

            if len == 0 {
                break;
            }

            complete_buf.extend_from_slice(&res_buf[..len]);
        }

        // let Some((v, _)) = serde_json_core::from_slice_escaped(complete_buf, escape_buf).ok() else {
        //     if let Err(e) = serde_json::from_slice::<TTCRes<'_>>(complete_buf) {
        //         log::error!("{e}");
        //     }

        //     return None;
        // };
        let Some(v) = serde_json::from_slice::<TTCRes<'_>>(complete_buf).ok() else {
            return None;
        };

        Some(v)
    }

    let base_path = "/service/publicJSONFeed?command=predictions&a=ttc&stopId=";
    let mut escape_buf = [0u8; 128];

    mbedtls_rs::sys::hook::backend::esp::digest::EspSha1::new();

    loop {
        let (req, res_slot) = rx.receive().await;

        log::info!("{req}");
        complete_buf.clear();

        let path = heapless::format!(152; "{base_path}{req}").unwrap();

        let res = embassy_time::with_timeout(
            Duration::from_secs(10),
            make_req(
                &mut complete_buf,
                &mut res_buf,
                &mut escape_buf,
                &mut conn,
                &path,
            ),
        )
        .await;
        let Ok(res) = res else {
            log::error!("timeout");
            continue;
        };

        let Some(res) = res else {
            log::error!("err");
            continue;
        };

        // log::info!("{res:#?}");
        let mut directions = heapless::Vec::<(&str, u16, &str), 16>::new();
        for prediction in res.predictions {
            let Some(direction) = prediction.direction else {
                continue;
            };
            let Some(actual_prediction) = direction.prediction.first() else {
                continue;
            };
            let Some(minutes) = actual_prediction.minutes.parse::<u16>().ok() else {
                continue;
            };

            directions.push((direction.title, minutes, actual_prediction.vehicle));
        }

        directions.sort_unstable_by_key(|v| v.1);

        let mut out = heapless::String::new();

        for (route, minutes, _vehicle) in directions {
            let minutes_plural = if minutes == 1 { "MINUTE" } else { "MINUTES" };
            writeln!(&mut out, "{route} - {minutes} {minutes_plural}");
        }

        res_slot.signal(out);

        // res_slot.signal(val);
    }

    // let client = DefaultHttpClient::new(&stack);
    // let mut response_buffer = [0u8; 8192];
    // let headers = [
    // HttpHeader::user_agent("Nanofish/0.9.1"),
    // ];
    // let v = client.get("https://webservices.umoiq.com/service/publicJSONFeed?command=predictions&a=ttc&stopId=9857", &headers, &mut response_buffer).await.unwrap();
    // log::info!("{}", v.0.body.as_str().unwrap());

    // let client = DefaultHttpClient::new(unsafe { core::ptr::NonNull::dangling().as_ref() });

    // let custom_headers = [
    //     HttpHeader { name: "X-Custom-Header", value: "custom-value" },
    //     HttpHeader::new(headers::ACCEPT, mime_types::JSON),
    // ];
    // let (response, bytes_read) = client.get(
    //     "http://example.com/api/status",
    //     &headers,
    //     &mut response_buffer
    // ).await?;

    //     while let Some(req) = rx.recv().await {
    //         let cmd = heapless::format!(128; "/service/publicJSONFeed?command=predictions&a=ttc&stopId={req}").unwrap();
    //         let Ok(res) = resource.get(&cmd).headers(&[("Host", "webservices.umoiq.com")]).send(&mut rx_buf).await else { continue };
    // //
    //         let Ok(res_body) = res.body().read_to_end().await else { continue };
    //         let Ok((v, _)) = serde_json_core::from_slice_escaped::<TTCRes<'_>>(res_body, &mut escape_buf) else {
    //             continue;
    //         };

    //         log::info!("{v:#?}");

    //     }
}

pub struct EspAccelQueueCustom;

impl EspAccelQueueCustom {
    fn start() -> Self {
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha1(Some(
                &mbedtls_rs::sys::hook::backend::esp::SHA1,
            ));
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha224(Some(
                &mbedtls_rs::sys::hook::backend::esp::SHA224,
            ));
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha256(Some(
                &mbedtls_rs::sys::hook::backend::esp::SHA256,
            ));
        }

        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha384(Some(
                &mbedtls_rs::sys::hook::backend::esp::SHA384,
            ));
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha512(Some(
                &mbedtls_rs::sys::hook::backend::esp::SHA512,
            ));
        }

        unsafe {
            mbedtls_rs::sys::hook::exp_mod::hook_exp_mod(Some(
                &mbedtls_rs::sys::hook::backend::esp::EXP_MOD,
            ));
        };

        Self
    }
}

impl Drop for EspAccelQueueCustom {
    fn drop(&mut self) {
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha1(None);
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha224(None);
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha256(None);
        }

        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha384(None);
        }
        unsafe {
            mbedtls_rs::sys::hook::digest::hook_sha512(None);
        }

        unsafe {
            mbedtls_rs::sys::hook::exp_mod::hook_exp_mod(None);
        };
    }
}
