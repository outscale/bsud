use clap::Parser;

static DEFAULT_CONFIG_PATH: &str = "/etc/osc/bsud.json";

pub fn parse() -> Args {
    Args::parse()
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about=None)]
pub struct Args {
    #[arg(long = "config", short = 'c', default_value_t = String::from(DEFAULT_CONFIG_PATH))]
    pub config_path: String,
}
