use crate::errors::Error;
use env_logger::Builder;
use log::LevelFilter;
use syslog::Facility;

#[derive(Debug, PartialEq, Eq)]
pub enum LogSystem {
    SysLog,
    StdErr,
}

fn get_loglevel(log_level: Option<&str>) -> Result<LevelFilter, Error> {
    let level = match log_level {
        Some(v) => match v {
            "error" => LevelFilter::Error,
            "warn" => LevelFilter::Warn,
            "info" => LevelFilter::Info,
            "debug" => LevelFilter::Debug,
            "trace" => LevelFilter::Trace,
            _ => {
                return Err(Error::new(&format!("{}: invalid log level", v)));
            }
        },
        None => crate::DEFAULT_LOG_LEVEL,
    };
    Ok(level)
}

fn set_log_syslog(log_level: LevelFilter) -> Result<(), Error> {
    syslog::init(Facility::LOG_DAEMON, log_level, Some(crate::APP_NAME))?;
    Ok(())
}

fn set_log_stderr(log_level: LevelFilter) -> Result<(), Error> {
    let mut builder = Builder::from_env("ACMED_LOG_LEVEL");
    builder.filter_level(log_level);
    builder.init();
    Ok(())
}

pub fn set_log_system(
    log_level: Option<&str>,
    has_syslog: bool,
    has_stderr: bool,
) -> Result<(LogSystem, LevelFilter), Error> {
    let log_level = get_loglevel(log_level)?;
    let mut logtype = crate::DEFAULT_LOG_SYSTEM;
    if has_stderr {
        logtype = LogSystem::StdErr;
    }
    if has_syslog {
        logtype = LogSystem::SysLog;
    }
    match logtype {
        LogSystem::SysLog => set_log_syslog(log_level)?,
        LogSystem::StdErr => set_log_stderr(log_level)?,
    };
    Ok((logtype, log_level))
}

#[cfg(test)]
mod tests {
    use super::set_log_system;

    #[test]
    fn test_invalid_level() {
        let ret = set_log_system(Some("invalid"), false, false);
        assert!(ret.is_err());
    }

    #[test]
    fn test_default_values() {
        let ret = set_log_system(None, false, false);
        assert!(ret.is_ok());
        let (logtype, log_level) = ret.unwrap();
        assert_eq!(logtype, crate::DEFAULT_LOG_SYSTEM);
        assert_eq!(log_level, crate::DEFAULT_LOG_LEVEL);
    }
}