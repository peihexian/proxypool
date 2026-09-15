use anyhow::{Context, Result};
use maxminddb::{geoip2, Reader};
use serde::Serialize;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

const COUNTRY_URLS: &[&str] = &[
    "https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-Country.mmdb",
    "https://cdn.jsdelivr.net/gh/P3TERX/GeoLite.mmdb@download/GeoLite2-Country.mmdb",
];
const ASN_URLS: &[&str] = &[
    "https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-ASN.mmdb",
    "https://cdn.jsdelivr.net/gh/P3TERX/GeoLite.mmdb@download/GeoLite2-ASN.mmdb",
];

const DC_KEYWORDS: &[&str] = &[
    "amazon", "aws", "google", "gcp", "microsoft", "azure", "digitalocean", "linode",
    "akamai", "cloudflare", "ovh", "hetzner", "vultr", "choopa", "leaseweb", "rackspace",
    "softlayer", "ibm cloud", "oracle", "alibaba", "aliyun", "tencent", "huawei", "ucloud",
    "contabo", "scaleway", "hostinger", "godaddy", "m247", "datacamp", "cdn77", "fastly",
    "stackpath", "zenlayer", "colocrossing", "psychz", "quadranet", "ovhcloud", "hetzner",
    "data center", "datacenter", "data centre", "colocation", "dedicated", "vps ", "hosting",
    "cloud platform", "server farm", "online sas",
];

const RESIDENTIAL_KEYWORDS: &[&str] = &[
    "comcast", "xfinity", "verizon", "at&t", "att ", "spectrum", "charter", "cox communication",
    "centurylink", "lumen", "frontier", "windstream", "optimum", "altice", "t-mobile",
    "us cellular", "mediacom", "wideopenwest", "astound", "google fiber", "bellsouth",
    "deutsche telekom", "vodafone", "telefonica", "telecom italia", "virgin media",
    "sky uk", "kpn", "proximus", "telstra", "optus", "rogers", "bell canada", "telus",
    "china telecom", "china unicom", "china mobile", "ntt communications", "softbank",
    "broadband", "cable", "dsl", "ftth", "fiber to the", "mobile network", "isp",
    "residential", "communications",
];

#[derive(Debug, Clone, Default, Serialize)]
pub struct GeoInfo {
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub asn: Option<i64>,
    pub asn_org: Option<String>,
    pub is_residential: bool,
}

pub struct GeoDb {
    inner: RwLock<Inner>,
    dir: PathBuf,
}

struct Inner {
    country: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
}

impl GeoDb {
    pub fn new(dir: PathBuf) -> Self {
        let db = Self {
            inner: RwLock::new(Inner {
                country: None,
                asn: None,
            }),
            dir,
        };
        let _ = db.reload();
        db
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn reload(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let country = load_reader(&self.dir.join("GeoLite2-Country.mmdb"));
        let asn = load_reader(&self.dir.join("GeoLite2-ASN.mmdb"));
        let mut g = self.inner.write().unwrap();
        g.country = country;
        g.asn = asn;
        Ok(())
    }

    pub fn lookup(&self, ip: IpAddr) -> GeoInfo {
        let g = self.inner.read().unwrap();
        let mut info = GeoInfo::default();
        if let Some(reader) = g.country.as_ref() {
            if let Ok(rec) = reader.lookup::<geoip2::Country>(ip) {
                if let Some(c) = rec.country {
                    info.country_code = c.iso_code.map(|s: &str| s.to_string());
                    info.country = c.names.and_then(|n| {
                        n.get("zh-CN")
                            .copied()
                            .or_else(|| n.get("en").copied())
                            .map(|s: &str| s.to_string())
                    });
                }
            }
        }
        if let Some(reader) = g.asn.as_ref() {
            if let Ok(rec) = reader.lookup::<geoip2::Asn>(ip) {
                info.asn = rec.autonomous_system_number.map(|n| n as i64);
                info.asn_org = rec
                    .autonomous_system_organization
                    .map(|s: &str| s.to_string());
            }
        }
        info.is_residential = classify_residential(info.asn_org.as_deref());
        info
    }

    pub fn lookup_str(&self, ip: &str) -> Option<GeoInfo> {
        ip.parse::<IpAddr>().ok().map(|ip| self.lookup(ip))
    }

    pub fn country_mtime(&self) -> Option<String> {
        file_mtime(&self.dir.join("GeoLite2-Country.mmdb"))
    }

    pub fn asn_mtime(&self) -> Option<String> {
        file_mtime(&self.dir.join("GeoLite2-ASN.mmdb"))
    }

    pub async fn update(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        download_first(COUNTRY_URLS, &self.dir.join("GeoLite2-Country.mmdb")).await?;
        download_first(ASN_URLS, &self.dir.join("GeoLite2-ASN.mmdb")).await?;
        self.reload()?;
        Ok(())
    }
}

fn load_reader(path: &Path) -> Option<Reader<Vec<u8>>> {
    Reader::open_readfile(path).ok()
}

fn file_mtime(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339())
}

fn classify_residential(org: Option<&str>) -> bool {
    let Some(org) = org else {
        return false;
    };
    let lower = org.to_ascii_lowercase();
    if DC_KEYWORDS.iter().any(|k| lower.contains(k)) {
        return false;
    }
    RESIDENTIAL_KEYWORDS.iter().any(|k| lower.contains(k))
}

async fn download_first(urls: &[&str], dest: &Path) -> Result<()> {
    let mut last = anyhow::anyhow!("no url");
    for url in urls {
        match download(url, dest).await {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last).context("下载 GeoIP 数据库失败")
}

async fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    if bytes.len() < 1024 * 50 {
        anyhow::bail!("下载文件过小，可能不是有效 MMDB");
    }
    let tmp = dest.with_extension("mmdb.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(tmp, dest)?;
    Ok(())
}
