use crate::{cli::cli_types::*, errors::CLIError};

use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};

pub trait CLI {
    type Options;
    type Output;

    const ABOUT: AboutType;
    const ARGUMENTS: &'static [ArgumentType];
    const FLAGS: &'static [FlagType];
    const NAME: NameType;
    const OPTIONS: &'static [OptionType];
    const SUBCOMMANDS: &'static [SubCommandType];

    #[cfg_attr(tarpaulin, skip)]
    fn new<'a, 'b>() -> App<'a, 'b> {
        let arguments = &Self::ARGUMENTS
            .iter()
            .map(|a| {
                let mut args = Arg::with_name(a.0).help(a.1).required(a.3).index(a.4);
                if a.2.len() > 0 {
                    args = args.possible_values(a.2);
                }
                args
            })
            .collect::<Vec<Arg<'static, 'static>>>();
        let flags = &Self::FLAGS
            .iter()
            .map(|a| Arg::from_usage(a))
            .collect::<Vec<Arg<'static, 'static>>>();
        let options = &Self::OPTIONS
            .iter()
            .map(|a| match a.2.len() > 0 {
                true => Arg::from_usage(a.0)
                    .conflicts_with_all(a.1)
                    .possible_values(a.2)
                    .requires_all(a.3),
                false => Arg::from_usage(a.0).conflicts_with_all(a.1).requires_all(a.3),
            })
            .collect::<Vec<Arg<'static, 'static>>>();
        let subcommands = Self::SUBCOMMANDS
            .iter()
            .map(|s| {
                SubCommand::with_name(s.0)
                    .about(s.1)
                    .args(
                        &s.2.iter()
                            .map(|a| {
                                let mut args = Arg::with_name(a.0).help(a.1).required(a.3).index(a.4);
                                if a.2.len() > 0 {
                                    args = args.possible_values(a.2);
                                }
                                args
                            })
                            .collect::<Vec<Arg<'static, 'static>>>(),
                    )
                    .args(
                        &s.3.iter()
                            .map(|a| Arg::from_usage(a))
                            .collect::<Vec<Arg<'static, 'static>>>(),
                    )
                    .args(
                        &s.4.iter()
                            .map(|a| match a.2.len() > 0 {
                                true => Arg::from_usage(a.0)
                                    .conflicts_with_all(a.1)
                                    .possible_values(a.2)
                                    .requires_all(a.3),
                                false => Arg::from_usage(a.0).conflicts_with_all(a.1).requires_all(a.3),
                            })
                            .collect::<Vec<Arg<'static, 'static>>>(),
                    )
                    .settings(s.5)
            })
            .collect::<Vec<App<'static, 'static>>>();

        SubCommand::with_name(Self::NAME)
            .about(Self::ABOUT)
            .settings(&[
                AppSettings::ColoredHelp,
                AppSettings::DisableHelpSubcommand,
                AppSettings::DisableVersion,
            ])
            .args(arguments)
            .args(flags)
            .args(options)
            .subcommands(subcommands)
    }

    #[cfg_attr(tarpaulin, skip)]
    fn process(arguments: &ArgMatches) -> Result<Self::Output, CLIError> {
        let options = Self::parse(arguments)?;
        let output = Self::output(options)?;
        Ok(output)
    }

    #[cfg_attr(tarpaulin, skip)]
    fn parse(arguments: &ArgMatches) -> Result<Self::Options, CLIError>;

    #[cfg_attr(tarpaulin, skip)]
    fn output(options: Self::Options) -> Result<Self::Output, CLIError>;
}