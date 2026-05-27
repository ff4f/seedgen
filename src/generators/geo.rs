use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

pub struct LatitudeGenerator;
impl Generator for LatitudeGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let v: f64 = rng.gen_range(-90.0..=90.0);
        Value::Float((v * 1_000_000.0).round() / 1_000_000.0)
    }
    fn name(&self) -> &str {
        "latitude"
    }
}

pub struct LongitudeGenerator;
impl Generator for LongitudeGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let v: f64 = rng.gen_range(-180.0..=180.0);
        Value::Float((v * 1_000_000.0).round() / 1_000_000.0)
    }
    fn name(&self) -> &str {
        "longitude"
    }
}

pub struct CityGenerator;
impl Generator for CityGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(CITIES[rng.gen_range(0..CITIES.len())].to_string())
    }
    fn name(&self) -> &str {
        "city"
    }
}

pub struct CountryGenerator;
impl Generator for CountryGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(COUNTRIES[rng.gen_range(0..COUNTRIES.len())].to_string())
    }
    fn name(&self) -> &str {
        "country"
    }
}

pub struct CountryCodeGenerator;
impl Generator for CountryCodeGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(COUNTRY_CODES[rng.gen_range(0..COUNTRY_CODES.len())].to_string())
    }
    fn name(&self) -> &str {
        "country_code"
    }
}

pub struct PostalCodeGenerator;
impl Generator for PostalCodeGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let n: u32 = rng.gen_range(0..100_000);
        Value::String(format!("{n:05}"))
    }
    fn name(&self) -> &str {
        "postal_code"
    }
}

pub struct StreetAddressGenerator;
impl Generator for StreetAddressGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let number: u32 = rng.gen_range(1..10_000);
        let street = STREET_NAMES[rng.gen_range(0..STREET_NAMES.len())];
        let suffix = STREET_SUFFIXES[rng.gen_range(0..STREET_SUFFIXES.len())];
        Value::String(format!("{number} {street} {suffix}"))
    }
    fn name(&self) -> &str {
        "street_address"
    }
}

const CITIES: &[&str] = &[
    "New York",
    "Los Angeles",
    "Chicago",
    "Houston",
    "Phoenix",
    "Philadelphia",
    "San Antonio",
    "San Diego",
    "Dallas",
    "San Jose",
    "Austin",
    "Jacksonville",
    "Fort Worth",
    "Columbus",
    "Charlotte",
    "San Francisco",
    "Indianapolis",
    "Seattle",
    "Denver",
    "Washington",
    "Boston",
    "El Paso",
    "Nashville",
    "Detroit",
    "Oklahoma City",
    "Portland",
    "Las Vegas",
    "Memphis",
    "Louisville",
    "Baltimore",
    "Milwaukee",
    "Albuquerque",
    "Tucson",
    "Fresno",
    "Sacramento",
    "Kansas City",
    "Mesa",
    "Atlanta",
    "Omaha",
    "Colorado Springs",
    "Raleigh",
    "Long Beach",
    "Virginia Beach",
    "Miami",
    "Oakland",
    "Minneapolis",
    "Tulsa",
    "Bakersfield",
    "Wichita",
    "Arlington",
    "London",
    "Manchester",
    "Birmingham",
    "Liverpool",
    "Edinburgh",
    "Glasgow",
    "Bristol",
    "Leeds",
    "Sheffield",
    "Paris",
    "Marseille",
    "Lyon",
    "Toulouse",
    "Nice",
    "Nantes",
    "Strasbourg",
    "Bordeaux",
    "Berlin",
    "Munich",
    "Hamburg",
    "Cologne",
    "Frankfurt",
    "Stuttgart",
    "Dusseldorf",
    "Dortmund",
    "Madrid",
    "Barcelona",
    "Valencia",
    "Seville",
    "Rome",
    "Milan",
    "Naples",
    "Turin",
    "Florence",
    "Tokyo",
    "Osaka",
    "Yokohama",
    "Nagoya",
    "Sapporo",
    "Kobe",
    "Kyoto",
    "Fukuoka",
    "Beijing",
    "Shanghai",
    "Guangzhou",
    "Shenzhen",
    "Tianjin",
    "Wuhan",
    "Chengdu",
    "Chongqing",
    "Mumbai",
    "Delhi",
    "Bangalore",
    "Hyderabad",
    "Chennai",
    "Kolkata",
    "Pune",
    "Sydney",
    "Melbourne",
    "Brisbane",
    "Perth",
    "Adelaide",
    "Toronto",
    "Montreal",
    "Vancouver",
    "Calgary",
    "Ottawa",
    "Mexico City",
    "Guadalajara",
    "Monterrey",
    "Sao Paulo",
    "Rio de Janeiro",
    "Brasilia",
    "Salvador",
    "Buenos Aires",
    "Cordoba",
    "Rosario",
    "Lima",
    "Bogota",
    "Santiago",
    "Caracas",
    "Cairo",
    "Alexandria",
    "Lagos",
    "Nairobi",
    "Johannesburg",
    "Cape Town",
    "Istanbul",
    "Ankara",
    "Tehran",
    "Riyadh",
    "Dubai",
    "Abu Dhabi",
    "Singapore",
    "Bangkok",
    "Kuala Lumpur",
    "Manila",
    "Jakarta",
    "Hanoi",
    "Ho Chi Minh City",
    "Seoul",
    "Busan",
    "Moscow",
    "Saint Petersburg",
];

const COUNTRIES: &[&str] = &[
    "United States",
    "United Kingdom",
    "Canada",
    "Australia",
    "Germany",
    "France",
    "Italy",
    "Spain",
    "Portugal",
    "Netherlands",
    "Belgium",
    "Switzerland",
    "Austria",
    "Sweden",
    "Norway",
    "Denmark",
    "Finland",
    "Iceland",
    "Ireland",
    "Poland",
    "Czechia",
    "Slovakia",
    "Hungary",
    "Romania",
    "Bulgaria",
    "Greece",
    "Croatia",
    "Slovenia",
    "Estonia",
    "Latvia",
    "Lithuania",
    "Ukraine",
    "Russia",
    "Belarus",
    "Turkey",
    "Israel",
    "Saudi Arabia",
    "United Arab Emirates",
    "Qatar",
    "Kuwait",
    "Egypt",
    "Morocco",
    "Algeria",
    "Tunisia",
    "South Africa",
    "Nigeria",
    "Kenya",
    "Ethiopia",
    "Ghana",
    "Tanzania",
    "China",
    "Japan",
    "South Korea",
    "India",
    "Pakistan",
    "Bangladesh",
    "Sri Lanka",
    "Nepal",
    "Indonesia",
    "Malaysia",
    "Singapore",
    "Thailand",
    "Vietnam",
    "Philippines",
    "Cambodia",
    "Laos",
    "Myanmar",
    "New Zealand",
    "Fiji",
    "Mexico",
    "Guatemala",
    "Cuba",
    "Jamaica",
    "Haiti",
    "Dominican Republic",
    "Costa Rica",
    "Panama",
    "Brazil",
    "Argentina",
    "Chile",
    "Peru",
    "Colombia",
    "Venezuela",
    "Ecuador",
    "Bolivia",
    "Uruguay",
    "Paraguay",
];

const COUNTRY_CODES: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];

const STREET_NAMES: &[&str] = &[
    "Main",
    "Oak",
    "Pine",
    "Maple",
    "Cedar",
    "Elm",
    "Washington",
    "Lincoln",
    "Jefferson",
    "Madison",
    "Park",
    "Lake",
    "Hill",
    "River",
    "Spring",
    "Summer",
    "Winter",
    "North",
    "South",
    "East",
    "West",
    "Sunset",
    "Highland",
    "Forest",
    "Meadow",
    "Valley",
    "Birch",
    "Walnut",
    "Chestnut",
    "Cherry",
    "Willow",
    "Poplar",
    "Sycamore",
    "Magnolia",
    "Dogwood",
    "Aspen",
    "Cypress",
    "Redwood",
    "Beech",
    "Holly",
    "Juniper",
    "Ash",
    "Hickory",
    "Linden",
    "Olive",
    "Bay",
    "Cove",
    "Harbor",
    "Ridge",
    "Crescent",
];

const STREET_SUFFIXES: &[&str] = &[
    "St", "Ave", "Blvd", "Rd", "Ln", "Dr", "Ct", "Way", "Pl", "Ter", "Pkwy",
];

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng(seed: u64) -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(seed)
    }

    fn s(v: Value) -> String {
        match v {
            Value::String(s) => s,
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn test_cities_pool_at_least_100() {
        assert!(CITIES.len() >= 100, "got {}", CITIES.len());
    }

    #[test]
    fn test_country_codes_are_all_two_letters() {
        for code in COUNTRY_CODES {
            assert_eq!(code.len(), 2, "{code} is not 2 chars");
            assert!(
                code.chars().all(|c| c.is_ascii_uppercase()),
                "{code} not uppercase"
            );
        }
    }

    #[test]
    fn test_country_codes_includes_known_real_codes() {
        // Sanity-check that we're using a real ISO list, not made-up codes.
        for known in ["US", "GB", "JP", "DE", "FR", "BR", "ID", "ZA", "CN", "AU"] {
            assert!(COUNTRY_CODES.contains(&known), "missing real code {known}");
        }
    }

    fn assert_deterministic<G: Generator>(g: &G) {
        let mut a = rng(42);
        let mut b = rng(42);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b), "{}", g.name());
        }
    }

    #[test]
    fn test_latitude_is_deterministic() {
        assert_deterministic(&LatitudeGenerator);
    }

    #[test]
    fn test_latitude_in_range() {
        let g = LatitudeGenerator;
        let mut r = rng(1);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Float(v) => assert!((-90.0..=90.0).contains(&v), "{v}"),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_longitude_is_deterministic() {
        assert_deterministic(&LongitudeGenerator);
    }

    #[test]
    fn test_longitude_in_range() {
        let g = LongitudeGenerator;
        let mut r = rng(1);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Float(v) => assert!((-180.0..=180.0).contains(&v), "{v}"),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_city_is_deterministic() {
        assert_deterministic(&CityGenerator);
    }

    #[test]
    fn test_city_is_from_pool() {
        let g = CityGenerator;
        let mut r = rng(1);
        let c = s(g.generate(&mut r));
        assert!(CITIES.contains(&c.as_str()), "{c} not in pool");
    }

    #[test]
    fn test_country_is_deterministic() {
        assert_deterministic(&CountryGenerator);
    }

    #[test]
    fn test_country_code_is_deterministic() {
        assert_deterministic(&CountryCodeGenerator);
    }

    #[test]
    fn test_country_code_format() {
        let g = CountryCodeGenerator;
        let mut r = rng(1);
        for _ in 0..20 {
            let c = s(g.generate(&mut r));
            assert_eq!(c.len(), 2);
            assert!(c.chars().all(|ch| ch.is_ascii_uppercase()));
        }
    }

    #[test]
    fn test_postal_code_is_five_digits() {
        let g = PostalCodeGenerator;
        let mut r = rng(1);
        for _ in 0..30 {
            let p = s(g.generate(&mut r));
            assert_eq!(p.len(), 5, "got {p:?}");
            assert!(p.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn test_postal_code_is_deterministic() {
        assert_deterministic(&PostalCodeGenerator);
    }

    #[test]
    fn test_street_address_is_deterministic() {
        assert_deterministic(&StreetAddressGenerator);
    }

    #[test]
    fn test_street_address_format() {
        let g = StreetAddressGenerator;
        let mut r = rng(1);
        let a = s(g.generate(&mut r));
        let parts: Vec<&str> = a.splitn(3, ' ').collect();
        assert_eq!(parts.len(), 3, "got {a:?}");
        assert!(parts[0].parse::<u32>().is_ok(), "house number bad: {a}");
        assert!(STREET_SUFFIXES.contains(&parts[2]), "suffix bad: {a}");
    }
}
