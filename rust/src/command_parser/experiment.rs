use std::{num::ParseIntError, sync::OnceLock, time::Duration};

use clap::Subcommand;
use strum::{Display, EnumIter, IntoEnumIterator};

#[derive(Debug, Clone, Display, Subcommand, EnumIter)]
pub enum Experiment {
    CounterRatioOneThree,
    CounterSubversion,
    CounterRatioOneThreeNonCS,
    CounterProportional {
        #[arg(value_parser = parse_duration, long = "cs", default_value = "100000", value_delimiter = ',')]
        cs_durations: Vec<Duration>,
        #[arg(value_parser = parse_duration, long = "non-cs", default_value = "0", value_delimiter = ',')]
        non_cs_durations: Vec<Duration>,
        #[arg(long = "file-name", default_value = "proportional_counter")]
        file_name: String,
    },
    ResponseTimeSingleAddition,
    ResponseTimeRatioOneThree,
}

impl Experiment {
    pub fn to_vec_ref() -> Vec<&'static Self> {
        static INSTANCE: OnceLock<Vec<Experiment>> = OnceLock::new();

        INSTANCE
            .get_or_init(|| Experiment::iter().collect())
            .iter()
            .collect()
    }
}

fn parse_duration(arg: &str) -> Result<Duration, ParseIntError> {
    let nanos = arg.parse::<u64>()?;
    Ok(Duration::from_nanos(nanos))
}
