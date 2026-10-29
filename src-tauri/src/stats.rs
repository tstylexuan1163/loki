//! Server resource readings for the terminal status bar.
//!
//! One short POSIX `sh` script runs on a fresh exec channel and prints
//! `key value...` lines. CPU and network rates need two samples, so the script
//! sleeps half a second between them. Unknown systems print nothing and the
//! reading comes back empty rather than wrong.

use serde::Serialize;

const SCRIPT: &str = r#"
os=$(uname -s 2>/dev/null)
if [ "$os" = Linux ]; then
  read _ a b c d e f g h _ < /proc/stat
  t1=$((a+b+c+d+e+f+g+h)); i1=$((d+e))
  n1=$(awk 'NR>2{r+=$2;t+=$10}END{print r, t}' /proc/net/dev)
  sleep 0.5
  read _ a b c d e f g h _ < /proc/stat
  t2=$((a+b+c+d+e+f+g+h)); i2=$((d+e))
  n2=$(awk 'NR>2{r+=$2;t+=$10}END{print r, t}' /proc/net/dev)
  echo "cpu $((t2-t1)) $((i2-i1))"
  echo "net $n1 $n2"
  echo "cpus $(grep -c ^processor /proc/cpuinfo)"
  awk '/^MemTotal/{t=$2}/^MemAvailable/{a=$2}END{print "mem", t*1024, (t-a)*1024}' /proc/meminfo
  echo "load $(cut -d' ' -f1-3 /proc/loadavg)"
  echo "uptime $(cut -d. -f1 /proc/uptime)"
elif [ "$os" = Darwin ]; then
  n=$(sysctl -n hw.ncpu)
  echo "cpus $n"
  echo "cpupct $(ps -A -o %cpu= | awk -v n="$n" '{s+=$1}END{printf "%d", s/n}')"
  ps_=$(sysctl -n hw.pagesize)
  used=$(vm_stat | awk -v p="$ps_" '/Pages active/{a=$3}/Pages wired/{w=$4}/occupied by compressor/{c=$5}END{gsub(/\./,"",a);gsub(/\./,"",w);gsub(/\./,"",c);print (a+w+c)*p}')
  echo "mem $(sysctl -n hw.memsize) $used"
  echo "load $(sysctl -n vm.loadavg | tr -d '{}')"
  b=$(sysctl -n kern.boottime | sed 's/.*sec = \([0-9]*\).*/\1/')
  echo "uptime $(( $(date +%s) - b ))"
fi
df -Pk / 2>/dev/null | awk 'NR==2{print "disk", $2*1024, $3*1024}'
"#;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStats {
    /// 0–100.
    pub cpu_percent: Option<f64>,
    pub cpus: Option<u32>,
    pub mem_total: Option<u64>,
    pub mem_used: Option<u64>,
    pub disk_total: Option<u64>,
    pub disk_used: Option<u64>,
    pub load: Option<[f64; 3]>,
    pub uptime_secs: Option<u64>,
    /// Bytes per second over the sampling window.
    pub net_rx_rate: Option<f64>,
    pub net_tx_rate: Option<f64>,
}

pub fn parse(output: &str) -> ServerStats {
    let mut s = ServerStats::default();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let nums: Vec<f64> = parts.filter_map(|p| p.parse().ok()).collect();
        match (key, nums.as_slice()) {
            ("cpu", [total, idle]) if *total > 0.0 => {
                s.cpu_percent = Some(((total - idle) / total * 100.0).clamp(0.0, 100.0));
            }
            ("cpupct", [p]) => s.cpu_percent = Some(p.clamp(0.0, 100.0)),
            ("cpus", [n]) => s.cpus = Some(*n as u32),
            ("mem", [total, used]) => {
                s.mem_total = Some(*total as u64);
                s.mem_used = Some(*used as u64);
            }
            ("disk", [total, used]) => {
                s.disk_total = Some(*total as u64);
                s.disk_used = Some(*used as u64);
            }
            ("load", [a, b, c]) => s.load = Some([*a, *b, *c]),
            ("uptime", [u]) => s.uptime_secs = Some(*u as u64),
            // Two samples half a second apart.
            ("net", [r1, t1, r2, t2]) => {
                s.net_rx_rate = Some(((r2 - r1) * 2.0).max(0.0));
                s.net_tx_rate = Some(((t2 - t1) * 2.0).max(0.0));
            }
            _ => {}
        }
    }
    s
}

/// Takes one reading over the session's connection.
pub async fn sample(session: &crate::ssh::LiveSession) -> Result<ServerStats, String> {
    let out = crate::ssh::exec_with_timeout(
        session,
        &format!("sh -c '{}'", SCRIPT.replace('\'', "'\\''")),
        std::time::Duration::from_secs(6),
        16 * 1024,
    )
    .await?;
    Ok(parse(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_linux_reading() {
        let out = "cpu 200 150\nnet 1000 500 2000 900\ncpus 4\nmem 8000 2000\nload 0.10 0.20 0.30\nuptime 3600\ndisk 100 40\n";
        let s = parse(out);
        assert_eq!(s.cpu_percent, Some(25.0));
        assert_eq!(s.cpus, Some(4));
        assert_eq!(s.mem_used, Some(2000));
        assert_eq!(s.disk_total, Some(100));
        assert_eq!(s.load, Some([0.1, 0.2, 0.3]));
        assert_eq!(s.uptime_secs, Some(3600));
        assert_eq!(s.net_rx_rate, Some(2000.0));
        assert_eq!(s.net_tx_rate, Some(800.0));
    }

    #[test]
    fn garbage_yields_an_empty_reading() {
        assert_eq!(parse("Welcome to the server!\n"), ServerStats::default());
    }

    #[cfg(unix)]
    #[test]
    fn script_runs_on_this_machine() {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(SCRIPT)
            .output()
            .expect("sh");
        let s = parse(&String::from_utf8_lossy(&out.stdout));
        assert!(s.cpus.unwrap_or(0) > 0, "{s:?}");
        assert!(s.mem_total.unwrap_or(0) > 0, "{s:?}");
        assert!(s.disk_total.unwrap_or(0) > 0, "{s:?}");
    }
}
