use std::io::Write;

#[derive(Default, Clone, PartialEq)]
pub struct ConnectionStatistics {
    pub connection_new_count: i64,
    pub connection_living_count: i64,
    pub connection_connecting_count: i64,
    pub request_initiated_count: i64,
    pub request_pending_count: i64,
    pub request_sent_count: i64,
    pub response_ok_count: i64,
    pub response_bad_count: i64,
}

pub struct ConnectionStatisticsEntry {
    pub time: std::time::SystemTime,
    pub stat: ConnectionStatistics,
}

impl ConnectionStatisticsEntry {
    pub fn write_csv_headers(mut write: impl Write) -> std::io::Result<()> {
        write.write_fmt(format_args!(
            "{}, {}, {}, {}, {}, {}, {}, {}, {}\n",
            "time",
            "connection_new_count",
            "connection_living_count",
            "connection_connecting_count",
            "request_initiated_count",
            "request_pending_count",
            "request_sent_count",
            "response_ok_count",
            "response_bad_count",
        ))?;
        Ok(())
    }
    pub fn write_csv_line(&self, mut write: impl Write) -> std::io::Result<()> {
        write.write_fmt(format_args!(
            "{}, {}, {}, {}, {}, {}, {}, {}, {}\n",
            self.time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros(),
            self.stat.connection_new_count,
            self.stat.connection_living_count,
            self.stat.connection_connecting_count,
            self.stat.request_initiated_count,
            self.stat.request_pending_count,
            self.stat.request_sent_count,
            self.stat.response_ok_count,
            self.stat.response_bad_count,
        ))?;
        Ok(())
    }
}
