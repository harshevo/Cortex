//! What can be watched: the exchange segments, a few instruments known
//! by heart, and Dhan's full list, read from its scrip master file.

use serde::{Deserialize, Serialize};

use crate::clock::Session;

/// The scrip master, with one row per instrument.
pub const SCRIP_MASTER: &str = "https://images.dhan.co/api-data/api-scrip-master.csv";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Segment {
    #[serde(rename = "IDX_I")]
    Index,
    #[serde(rename = "NSE_EQ")]
    NseEquity,
    #[serde(rename = "NSE_FNO")]
    NseDerivatives,
    #[serde(rename = "NSE_CURRENCY")]
    NseCurrency,
    #[serde(rename = "BSE_EQ")]
    BseEquity,
    #[serde(rename = "MCX_COMM")]
    Commodity,
    #[serde(rename = "BSE_CURRENCY")]
    BseCurrency,
    #[serde(rename = "BSE_FNO")]
    BseDerivatives,
}

impl Segment {
    /// Dhan's name, as the REST API takes it.
    pub fn code(self) -> &'static str {
        match self {
            Segment::Index => "IDX_I",
            Segment::NseEquity => "NSE_EQ",
            Segment::NseDerivatives => "NSE_FNO",
            Segment::NseCurrency => "NSE_CURRENCY",
            Segment::BseEquity => "BSE_EQ",
            Segment::Commodity => "MCX_COMM",
            Segment::BseCurrency => "BSE_CURRENCY",
            Segment::BseDerivatives => "BSE_FNO",
        }
    }

    /// The number the live feed calls it.
    pub fn number(self) -> u8 {
        match self {
            Segment::Index => 0,
            Segment::NseEquity => 1,
            Segment::NseDerivatives => 2,
            Segment::NseCurrency => 3,
            Segment::BseEquity => 4,
            Segment::Commodity => 5,
            Segment::BseCurrency => 7,
            Segment::BseDerivatives => 8,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "IDX_I" => Segment::Index,
            "NSE_EQ" => Segment::NseEquity,
            "NSE_FNO" => Segment::NseDerivatives,
            "NSE_CURRENCY" => Segment::NseCurrency,
            "BSE_EQ" => Segment::BseEquity,
            "MCX_COMM" => Segment::Commodity,
            "BSE_CURRENCY" => Segment::BseCurrency,
            "BSE_FNO" => Segment::BseDerivatives,
            _ => return None,
        })
    }

    /// A short name for a person: `NSE`, `BSE`, `MCX`, `INDEX`.
    pub fn exchange(self) -> &'static str {
        match self {
            Segment::Index => "INDEX",
            Segment::NseEquity | Segment::NseDerivatives | Segment::NseCurrency => "NSE",
            Segment::BseEquity | Segment::BseDerivatives | Segment::BseCurrency => "BSE",
            Segment::Commodity => "MCX",
        }
    }

    pub fn session(self) -> Session {
        if crate::clock::always_open() {
            return Session::ALWAYS;
        }

        match self {
            Segment::Commodity => Session::COMMODITY,
            Segment::NseCurrency | Segment::BseCurrency => Session::CURRENCY,
            _ => Session::EQUITY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Instrument {
    pub security_id: String,
    pub segment: Segment,

    /// Dhan's instrument type: `EQUITY`, `INDEX`, `FUTIDX` and so on.
    pub kind: String,

    /// As traded: `RELIANCE`, `NIFTY`.
    pub symbol: String,

    /// As shown: `Reliance Industries`, `Nifty 50`.
    pub name: String,
}

impl Instrument {
    fn known(id: &str, segment: Segment, kind: &str, symbol: &str, name: &str) -> Self {
        Self {
            security_id: id.to_string(),
            segment,
            kind: kind.to_string(),
            symbol: symbol.to_string(),
            name: name.to_string(),
        }
    }

    /// Identifies it across runs: `NSE_EQ:2885`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.segment.code(), self.security_id)
    }

    /// Indices cannot be traded and have no volume or order book.
    pub fn is_index(&self) -> bool {
        self.segment == Segment::Index
    }
}

/// A few instruments, so that something can be watched before Dhan's
/// full list has been fetched.
pub fn favourites() -> Vec<Instrument> {
    use Segment::*;

    vec![
        Instrument::known("13", Index, "INDEX", "NIFTY", "Nifty 50"),
        Instrument::known("25", Index, "INDEX", "BANKNIFTY", "Nifty Bank"),
        Instrument::known("27", Index, "INDEX", "FINNIFTY", "Nifty Financial Services"),
        Instrument::known("51", Index, "INDEX", "SENSEX", "BSE Sensex"),
        Instrument::known(
            "2885",
            NseEquity,
            "EQUITY",
            "RELIANCE",
            "Reliance Industries",
        ),
        Instrument::known("1333", NseEquity, "EQUITY", "HDFCBANK", "HDFC Bank"),
        Instrument::known("4963", NseEquity, "EQUITY", "ICICIBANK", "ICICI Bank"),
        Instrument::known(
            "11536",
            NseEquity,
            "EQUITY",
            "TCS",
            "Tata Consultancy Services",
        ),
        Instrument::known("1594", NseEquity, "EQUITY", "INFY", "Infosys"),
        Instrument::known("3045", NseEquity, "EQUITY", "SBIN", "State Bank of India"),
        Instrument::known("1660", NseEquity, "EQUITY", "ITC", "ITC"),
        Instrument::known("10604", NseEquity, "EQUITY", "BHARTIARTL", "Bharti Airtel"),
        Instrument::known("11483", NseEquity, "EQUITY", "LT", "Larsen & Toubro"),
        Instrument::known("5900", NseEquity, "EQUITY", "AXISBANK", "Axis Bank"),
        Instrument::known(
            "1922",
            NseEquity,
            "EQUITY",
            "KOTAKBANK",
            "Kotak Mahindra Bank",
        ),
    ]
}

/// Reads Dhan's compact scrip master. Options are left out: there are
/// hundreds of thousands of them and they are not charted here.
pub fn read_scrip_master(text: &str) -> Vec<Instrument> {
    let mut lines = text.lines();

    let Some(header) = lines.next() else {
        return Vec::new();
    };

    let columns: Vec<&str> = header.split(',').map(|name| name.trim()).collect();
    let at = |name: &str| columns.iter().position(|column| *column == name);

    let (Some(exchange), Some(segment), Some(id), Some(kind)) = (
        at("SEM_EXM_EXCH_ID"),
        at("SEM_SEGMENT"),
        at("SEM_SMST_SECURITY_ID"),
        at("SEM_INSTRUMENT_NAME"),
    ) else {
        return Vec::new();
    };

    let symbol = at("SEM_TRADING_SYMBOL");
    let custom = at("SEM_CUSTOM_SYMBOL");
    let name = at("SM_SYMBOL_NAME");
    let series = at("SEM_SERIES");

    let mut found = Vec::new();

    for line in lines {
        let fields: Vec<&str> = line.split(',').map(|field| field.trim()).collect();
        let field = |at: Option<usize>| at.and_then(|at| fields.get(at)).copied().unwrap_or("");

        let kind = field(Some(kind));

        if !matches!(
            kind,
            "EQUITY" | "INDEX" | "FUTIDX" | "FUTSTK" | "FUTCOM" | "FUTCUR"
        ) {
            continue;
        }

        // Cash shares other than the ordinary series are left out.
        if kind == "EQUITY" && !matches!(field(series), "EQ" | "A" | "B" | "X" | "") {
            continue;
        }

        let segment = match (field(Some(exchange)), field(Some(segment))) {
            (_, "I") => Segment::Index,
            ("NSE", "E") => Segment::NseEquity,
            ("NSE", "D") => Segment::NseDerivatives,
            ("NSE", "C") => Segment::NseCurrency,
            ("BSE", "E") => Segment::BseEquity,
            ("BSE", "D") => Segment::BseDerivatives,
            ("BSE", "C") => Segment::BseCurrency,
            ("MCX", "M") => Segment::Commodity,
            _ => continue,
        };

        let symbol = field(symbol);
        let shown = [field(custom), field(name), symbol]
            .into_iter()
            .find(|text| !text.is_empty() && *text != "NA")
            .unwrap_or(symbol);

        found.push(Instrument {
            security_id: field(Some(id)).to_string(),
            segment,
            kind: kind.to_string(),
            symbol: symbol.to_string(),
            name: shown.to_string(),
        });
    }

    found
}

/// The instruments that match `query` best, at most `most`: whole symbol
/// first, then symbols that begin with it, then names that hold it. Cash
/// shares and indices come before futures.
pub fn search(all: &[Instrument], query: &str, most: usize) -> Vec<Instrument> {
    let query = query.trim().to_uppercase();

    if query.is_empty() {
        return all.iter().take(most).cloned().collect();
    }

    let mut ranked: Vec<(u8, &Instrument)> = all
        .iter()
        .filter_map(|instrument| {
            let symbol = instrument.symbol.to_uppercase();
            let name = instrument.name.to_uppercase();

            let rank = if symbol == query {
                0
            } else if symbol.starts_with(&query) {
                1
            } else if name.contains(&query) || symbol.contains(&query) {
                2
            } else {
                return None;
            };

            let derivative = u8::from(instrument.kind.starts_with("FUT"));

            Some((rank * 2 + derivative, instrument))
        })
        .collect();

    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.symbol.len().cmp(&right.1.symbol.len()))
    });

    let mut seen = std::collections::HashSet::new();

    ranked
        .into_iter()
        .map(|(_, instrument)| instrument.clone())
        .filter(|instrument| seen.insert(instrument.key()))
        .take(most)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "SEM_EXM_EXCH_ID,SEM_SEGMENT,SEM_SMST_SECURITY_ID,SEM_INSTRUMENT_NAME,SEM_EXPIRY_CODE,SEM_TRADING_SYMBOL,SEM_LOT_UNITS,SEM_CUSTOM_SYMBOL,SEM_EXPIRY_DATE,SEM_STRIKE_PRICE,SEM_OPTION_TYPE,SEM_TICK_SIZE,SEM_EXPIRY_FLAG,SEM_EXCH_INSTRUMENT_TYPE,SEM_SERIES,SM_SYMBOL_NAME
NSE,E,2885,EQUITY,,RELIANCE,1.0,Reliance Industries,,,,10.0,NA,ES,EQ,RELIANCE INDUSTRIES LTD
NSE,I,13,INDEX,,NIFTY,1.0,Nifty 50,,,,0.05,NA,INDEX,NA,NIFTY
NSE,D,35001,OPTIDX,0,NIFTY-Oct2026-25000-CE,75.0,NIFTY 30 OCT 25000 CALL,2026-10-30,25000,CE,5.0,M,OP,NA,NIFTY
NSE,D,52000,FUTSTK,0,RELIANCE-Oct2026-FUT,500.0,RELIANCE OCT FUT,2026-10-30,,,10.0,M,FUT,NA,RELIANCE
NSE,E,9999,EQUITY,,SOMEBOND,1.0,Some Bond,,,,1.0,NA,ES,GB,SOME BOND
";

    #[test]
    fn the_master_is_read_without_options_or_bonds() {
        let all = read_scrip_master(MASTER);

        assert_eq!(all.len(), 3);
        assert_eq!(all[0].segment, Segment::NseEquity);
        assert_eq!(all[0].name, "Reliance Industries");
        assert_eq!(all[1].segment, Segment::Index);
        assert_eq!(all[2].kind, "FUTSTK");
    }

    #[test]
    fn search_puts_the_share_before_its_future() {
        let all = read_scrip_master(MASTER);
        let found = search(&all, "reliance", 10);

        assert_eq!(found[0].security_id, "2885");
        assert_eq!(found[1].security_id, "52000");
        assert!(search(&all, "zzz", 10).is_empty());
    }

    #[test]
    fn favourites_have_distinct_keys() {
        let favourites = favourites();
        let keys: std::collections::HashSet<String> =
            favourites.iter().map(Instrument::key).collect();

        assert_eq!(keys.len(), favourites.len());
    }
}
