use clap::Parser;

pub fn parse() -> Args {
    Args::parse()
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about=None)]
pub struct Args {
    #[arg(long = "config", short = 'c', default_value_t = String::from("/etc/osc/bsud/config.json"))]
    pub config_path: String,
}
