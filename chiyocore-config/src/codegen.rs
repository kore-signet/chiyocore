use genco::{lang::Rust, quote, quote_fn, quote_in, tokens::FormatInto};
use litemap::LiteMap;

use crate::{CompanionConfig, LayerConfig, PingBotConfig};

impl FormatInto<Rust> for CompanionConfig {
    fn format_into(self, tokens: &mut genco::Tokens<Rust>) {
        let CompanionConfig { id, tcp_port } = self;
        let id = id.to_string();
        quote_in!(*tokens =>
            chiyocore_config::CompanionConfig {
                id: Cow::Borrowed($[str]($[const](id))),
                tcp_port: $tcp_port
            }
        )
    }
}

impl FormatInto<Rust> for LayerConfig {
    fn format_into(self, tokens: &mut genco::Tokens<Rust>) {
        match self {
            LayerConfig::TTCBot => quote_in!(*tokens => ()),
            LayerConfig::PingBot(ping_cfg) => ping_cfg.format_into(tokens),
            LayerConfig::Companion(companion_config) => companion_config.format_into(tokens),
        }
    }
}

pub fn fmt_litemap(lm: LiteMap<String, String>) -> impl FormatInto<Rust> {
    let lm = lm
        .into_tuple_vec()
        .into_iter()
        .map(|(k, v)| quote! { ($[str]($[const](k)).into(), $[str]($[const](v)).into())});

    // LiteMap::from_i

    quote_fn! {
        litemap::LiteMap::from_iter([
            $(for kv in lm join (, ) => $kv)
        ])
    }
}

impl FormatInto<Rust> for PingBotConfig {
    fn format_into(self, tokens: &mut genco::Tokens<Rust>) {
        // PingBotConfig {
        // pub name: Cow<'static, str>,
        // pub channels: Cow<'static, [SmolStr]>
        // }
        let PingBotConfig { name, channels } = self;
        let channels = channels.into_owned().into_iter().map(|c| {
            let v = c.to_string();
            quote_fn! { $[str]($[const](v)).into() }
        });

        quote_in! { *tokens =>
            chiyocore_config::PingBotConfig {
                name: Cow::Borrowed($[str]($[const](name))),
                channels: Cow::Owned([
                    $(for c in channels join (, ) => $c)
                ].into())
            }
        }
    }
}
