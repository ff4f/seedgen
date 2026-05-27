use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

// ---------- generators ------------------------------------------------------

pub struct EmailGenerator;
impl Generator for EmailGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let first = pick(FIRST_NAMES, rng).to_ascii_lowercase();
        let last = pick(LAST_NAMES, rng).to_ascii_lowercase();
        let domain = pick(EMAIL_DOMAINS, rng);
        Value::String(format!("{first}.{last}@{domain}"))
    }
    fn name(&self) -> &str {
        "email"
    }
}

pub struct FirstNameGenerator;
impl Generator for FirstNameGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(pick(FIRST_NAMES, rng).to_string())
    }
    fn name(&self) -> &str {
        "first_name"
    }
}

pub struct LastNameGenerator;
impl Generator for LastNameGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(pick(LAST_NAMES, rng).to_string())
    }
    fn name(&self) -> &str {
        "last_name"
    }
}

pub struct FullNameGenerator;
impl Generator for FullNameGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(format!(
            "{} {}",
            pick(FIRST_NAMES, rng),
            pick(LAST_NAMES, rng)
        ))
    }
    fn name(&self) -> &str {
        "full_name"
    }
}

pub struct UsernameGenerator;
impl Generator for UsernameGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let first = pick(FIRST_NAMES, rng).to_ascii_lowercase();
        let digits: u32 = rng.gen_range(10..10_000);
        Value::String(format!("{first}{digits}"))
    }
    fn name(&self) -> &str {
        "username"
    }
}

pub struct PhoneGenerator;
impl Generator for PhoneGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let area: u32 = rng.gen_range(200..1000);
        let prefix: u32 = rng.gen_range(200..1000);
        let line: u32 = rng.gen_range(0..10_000);
        Value::String(format!("+1-{area:03}-{prefix:03}-{line:04}"))
    }
    fn name(&self) -> &str {
        "phone"
    }
}

pub struct UrlGenerator;
impl Generator for UrlGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let word = pick(LOREM_WORDS, rng);
        let tld = pick(URL_TLDS, rng);
        let path = pick(URL_PATHS, rng);
        Value::String(format!("https://{word}.{tld}/{path}"))
    }
    fn name(&self) -> &str {
        "url"
    }
}

pub struct PasswordGenerator;
impl Generator for PasswordGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let salt: String = (0..22).map(|_| pick_char(BCRYPT_CHARS, rng)).collect();
        let hash: String = (0..31).map(|_| pick_char(BCRYPT_CHARS, rng)).collect();
        Value::String(format!("$2b$10${salt}{hash}"))
    }
    fn name(&self) -> &str {
        "password"
    }
}

pub struct SlugGenerator;
impl Generator for SlugGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let parts: Vec<&str> = (0..3).map(|_| pick(LOREM_WORDS, rng)).collect();
        Value::String(parts.join("-"))
    }
    fn name(&self) -> &str {
        "slug"
    }
}

pub struct SentenceGenerator;
impl Generator for SentenceGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(generate_sentence(rng))
    }
    fn name(&self) -> &str {
        "sentence"
    }
}

pub struct ParagraphGenerator;
impl Generator for ParagraphGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let n: usize = rng.gen_range(2..=5);
        let sentences: Vec<String> = (0..n).map(|_| generate_sentence(rng)).collect();
        Value::String(sentences.join(" "))
    }
    fn name(&self) -> &str {
        "paragraph"
    }
}

pub struct CompanyNameGenerator;
impl Generator for CompanyNameGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let base = pick(COMPANY_BASES, rng);
        let suffix = pick(COMPANY_SUFFIXES, rng);
        Value::String(format!("{base} {suffix}"))
    }
    fn name(&self) -> &str {
        "company_name"
    }
}

pub struct AvatarUrlGenerator;
impl Generator for AvatarUrlGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let id: u32 = rng.gen_range(1..10_000);
        Value::String(format!("https://avatars.example.com/{id}.png"))
    }
    fn name(&self) -> &str {
        "avatar_url"
    }
}

// ---------- helpers ---------------------------------------------------------

fn pick<'a>(pool: &'a [&'a str], rng: &mut ChaCha8Rng) -> &'a str {
    let idx = rng.gen_range(0..pool.len());
    pool[idx]
}

fn pick_char(pool: &[u8], rng: &mut ChaCha8Rng) -> char {
    let idx = rng.gen_range(0..pool.len());
    pool[idx] as char
}

fn generate_sentence(rng: &mut ChaCha8Rng) -> String {
    let n: usize = rng.gen_range(5..=12);
    let mut words: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        let w = pick(LOREM_WORDS, rng);
        if i == 0 {
            let mut chars = w.chars();
            words.push(
                chars
                    .next()
                    .map(|c| c.to_ascii_uppercase().to_string())
                    .unwrap_or_default()
                    + chars.as_str(),
            );
        } else {
            words.push(w.to_string());
        }
    }
    format!("{}.", words.join(" "))
}

// ---------- data pools ------------------------------------------------------

const EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "yahoo.com",
    "hotmail.com",
    "outlook.com",
    "icloud.com",
    "protonmail.com",
    "fastmail.com",
    "hey.com",
    "example.com",
    "mail.com",
    "aol.com",
    "msn.com",
    "live.com",
    "duck.com",
    "tutanota.com",
];

const URL_TLDS: &[&str] = &[
    "com", "net", "org", "io", "dev", "app", "co", "xyz", "info", "biz", "tech", "ai",
];

const URL_PATHS: &[&str] = &[
    "", "about", "contact", "blog", "products", "services", "home", "help", "faq", "news",
    "careers", "pricing", "docs", "support", "login",
];

const BCRYPT_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789./";

const LOREM_WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "labore",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "ex",
    "commodo",
    "consequat",
    "duis",
    "aute",
    "irure",
    "reprehenderit",
    "voluptate",
    "velit",
    "esse",
    "cillum",
    "fugiat",
    "nulla",
    "pariatur",
    "excepteur",
    "sint",
    "occaecat",
    "cupidatat",
    "non",
    "proident",
    "sunt",
    "culpa",
    "qui",
    "officia",
    "deserunt",
    "mollit",
    "anim",
    "est",
    "laborum",
    "rerum",
    "natus",
    "voluptas",
    "accusantium",
    "doloremque",
    "totam",
    "aperiam",
    "inventore",
    "veritatis",
    "quasi",
    "architecto",
    "beatae",
    "vitae",
];

const COMPANY_SUFFIXES: &[&str] = &[
    "Inc",
    "LLC",
    "Corp",
    "Ltd",
    "Group",
    "Industries",
    "Holdings",
    "Partners",
    "Solutions",
    "Systems",
    "Networks",
    "Labs",
    "Studios",
    "Ventures",
    "Capital",
    "Technologies",
    "Software",
    "Services",
    "Consulting",
    "Enterprises",
];

const COMPANY_BASES: &[&str] = &[
    "Acme",
    "Globex",
    "Initech",
    "Umbrella",
    "Stark",
    "Wayne",
    "Cyberdyne",
    "Tyrell",
    "Soylent",
    "Aperture",
    "Hooli",
    "Vandelay",
    "Apex",
    "Vertex",
    "Zenith",
    "Nexus",
    "Echo",
    "Pulse",
    "Quantum",
    "Synergy",
    "Vortex",
    "Helix",
    "Catalyst",
    "Phoenix",
    "Atlas",
    "Orion",
    "Polaris",
    "Nova",
    "Lumen",
    "Stratus",
    "Cirrus",
    "Cascade",
    "Beacon",
    "Anchor",
    "Forge",
    "Summit",
    "Pioneer",
    "Crestline",
    "Northwind",
    "Silverlake",
];

const FIRST_NAMES: &[&str] = &[
    "Aaron",
    "Abigail",
    "Adam",
    "Adrian",
    "Aiden",
    "Alan",
    "Alex",
    "Alexa",
    "Alexander",
    "Alice",
    "Alicia",
    "Allison",
    "Amanda",
    "Amber",
    "Amelia",
    "Amir",
    "Amy",
    "Andrea",
    "Andrew",
    "Angela",
    "Angelica",
    "Anna",
    "Anthony",
    "Antonio",
    "April",
    "Ariana",
    "Arthur",
    "Ashley",
    "Audrey",
    "Ava",
    "Barbara",
    "Beatrice",
    "Benjamin",
    "Bernard",
    "Beth",
    "Beverly",
    "Blake",
    "Brad",
    "Brandon",
    "Brenda",
    "Brian",
    "Brittany",
    "Brooke",
    "Bruce",
    "Bryan",
    "Caleb",
    "Cameron",
    "Camila",
    "Carl",
    "Carlos",
    "Carmen",
    "Caroline",
    "Catherine",
    "Cecilia",
    "Charles",
    "Charlie",
    "Charlotte",
    "Chelsea",
    "Cheryl",
    "Chloe",
    "Chris",
    "Christian",
    "Christina",
    "Christine",
    "Christopher",
    "Cindy",
    "Claire",
    "Clara",
    "Claudia",
    "Cody",
    "Colin",
    "Connor",
    "Corey",
    "Craig",
    "Crystal",
    "Cynthia",
    "Daisy",
    "Dale",
    "Daniel",
    "Daniela",
    "Danielle",
    "Darren",
    "David",
    "Dean",
    "Deborah",
    "Denise",
    "Dennis",
    "Derek",
    "Diana",
    "Diane",
    "Diego",
    "Dominic",
    "Donald",
    "Donna",
    "Doris",
    "Dorothy",
    "Douglas",
    "Duncan",
    "Dylan",
    "Edith",
    "Edward",
    "Edwin",
    "Eileen",
    "Elaine",
    "Eleanor",
    "Eli",
    "Elijah",
    "Elisa",
    "Elizabeth",
    "Ella",
    "Ellen",
    "Emily",
    "Emma",
    "Eric",
    "Erica",
    "Erin",
    "Ethan",
    "Eugene",
    "Eva",
    "Evan",
    "Evelyn",
    "Faith",
    "Felix",
    "Fernando",
    "Fiona",
    "Frances",
    "Francis",
    "Frank",
    "Franklin",
    "Fred",
    "Gabriel",
    "Gabriela",
    "Gail",
    "Gary",
    "Genesis",
    "George",
    "Georgia",
    "Gerald",
    "Gloria",
    "Grace",
    "Grant",
    "Greg",
    "Hailey",
    "Hannah",
    "Harold",
    "Harper",
    "Harry",
    "Hazel",
    "Heather",
    "Hector",
    "Helen",
    "Henry",
    "Holly",
    "Hugo",
    "Ian",
    "Ingrid",
    "Irene",
    "Isaac",
    "Isabella",
    "Isaiah",
    "Ivan",
    "Ivy",
    "Jack",
    "Jackson",
    "Jacob",
    "Jacqueline",
    "Jada",
    "Jade",
    "Jaime",
    "James",
    "Jamie",
    "Jane",
    "Janet",
    "Janice",
    "Jared",
    "Jason",
    "Jasper",
    "Jay",
    "Jean",
    "Jeff",
    "Jennifer",
    "Jeremy",
    "Jerry",
    "Jesse",
    "Jessica",
    "Jesus",
    "Jill",
    "Jim",
    "Joan",
    "Joanna",
    "Joel",
    "John",
    "Johnny",
    "Jonathan",
    "Jordan",
    "Jorge",
    "Jose",
    "Joseph",
    "Joshua",
    "Joy",
    "Joyce",
    "Juan",
    "Judith",
    "Judy",
    "Julia",
    "Julian",
    "Julie",
    "June",
    "Justin",
    "Karen",
    "Karl",
    "Kate",
    "Katelyn",
    "Katherine",
    "Kathleen",
    "Kathryn",
    "Kathy",
    "Katie",
    "Kayla",
    "Keith",
    "Kelly",
    "Ken",
    "Kenneth",
    "Kevin",
    "Kim",
    "Kimberly",
];

const LAST_NAMES: &[&str] = &[
    "Smith",
    "Johnson",
    "Williams",
    "Brown",
    "Jones",
    "Garcia",
    "Miller",
    "Davis",
    "Rodriguez",
    "Martinez",
    "Hernandez",
    "Lopez",
    "Gonzalez",
    "Wilson",
    "Anderson",
    "Thomas",
    "Taylor",
    "Moore",
    "Jackson",
    "Martin",
    "Lee",
    "Perez",
    "Thompson",
    "White",
    "Harris",
    "Sanchez",
    "Clark",
    "Ramirez",
    "Lewis",
    "Robinson",
    "Walker",
    "Young",
    "Allen",
    "King",
    "Wright",
    "Scott",
    "Torres",
    "Nguyen",
    "Hill",
    "Flores",
    "Green",
    "Adams",
    "Nelson",
    "Baker",
    "Hall",
    "Rivera",
    "Campbell",
    "Mitchell",
    "Carter",
    "Roberts",
    "Gomez",
    "Phillips",
    "Evans",
    "Turner",
    "Diaz",
    "Parker",
    "Cruz",
    "Edwards",
    "Collins",
    "Reyes",
    "Stewart",
    "Morris",
    "Morales",
    "Murphy",
    "Cook",
    "Rogers",
    "Gutierrez",
    "Ortiz",
    "Morgan",
    "Cooper",
    "Peterson",
    "Bailey",
    "Reed",
    "Kelly",
    "Howard",
    "Ramos",
    "Kim",
    "Cox",
    "Ward",
    "Richardson",
    "Watson",
    "Brooks",
    "Chavez",
    "Wood",
    "James",
    "Bennett",
    "Gray",
    "Mendoza",
    "Ruiz",
    "Hughes",
    "Price",
    "Alvarez",
    "Castillo",
    "Sanders",
    "Patel",
    "Myers",
    "Long",
    "Ross",
    "Foster",
    "Jimenez",
    "Powell",
    "Jenkins",
    "Perry",
    "Russell",
    "Sullivan",
    "Bell",
    "Coleman",
    "Butler",
    "Henderson",
    "Barnes",
    "Gonzales",
    "Fisher",
    "Vasquez",
    "Simmons",
    "Romero",
    "Jordan",
    "Patterson",
    "Alexander",
    "Hamilton",
    "Graham",
    "Reynolds",
    "Griffin",
    "Wallace",
    "Moreno",
    "West",
    "Cole",
    "Hayes",
    "Bryant",
    "Herrera",
    "Gibson",
    "Ellis",
    "Tran",
    "Medina",
    "Aguilar",
    "Stevens",
    "Murray",
    "Ford",
    "Castro",
    "Marshall",
    "Owens",
    "Harrison",
    "Fernandez",
    "McDonald",
    "Woods",
    "Washington",
    "Kennedy",
    "Wells",
    "Vargas",
    "Henry",
    "Chen",
    "Freeman",
    "Webb",
    "Tucker",
    "Guzman",
    "Burns",
    "Crawford",
    "Olson",
    "Simpson",
    "Porter",
    "Hunter",
    "Gordon",
    "Mendez",
    "Silva",
    "Shaw",
    "Snyder",
    "Mason",
    "Dixon",
    "Munoz",
    "Hunt",
    "Hicks",
    "Holmes",
    "Palmer",
    "Wagner",
    "Black",
    "Robertson",
    "Boyd",
    "Rose",
    "Stone",
    "Salazar",
    "Fox",
    "Warren",
    "Mills",
    "Meyer",
    "Rice",
    "Schmidt",
    "Garza",
    "Daniels",
    "Ferguson",
    "Nichols",
    "Stephens",
    "Soto",
    "Weaver",
    "Ryan",
    "Gardner",
    "Payne",
    "Grant",
    "Dunn",
    "Kelley",
    "Spencer",
    "Hawkins",
    "Arnold",
    "Pierce",
    "Vazquez",
    "Hansen",
    "Peters",
    "Santos",
    "Hart",
    "Bradley",
    "Knight",
    "Elliott",
    "Cunningham",
    "Duncan",
    "Armstrong",
    "Hudson",
    "Carroll",
    "Lane",
    "Riley",
    "Andrews",
    "Alvarado",
    "Ray",
    "Delgado",
    "Berry",
    "Perkins",
    "Hoffman",
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
            other => panic!("expected Value::String, got {other:?}"),
        }
    }

    // --- pool size invariants -----------------------------------------------

    #[test]
    fn test_first_names_pool_at_least_200() {
        assert!(FIRST_NAMES.len() >= 200, "len was {}", FIRST_NAMES.len());
    }

    #[test]
    fn test_last_names_pool_at_least_200() {
        assert!(LAST_NAMES.len() >= 200, "len was {}", LAST_NAMES.len());
    }

    // --- determinism: per-generator -----------------------------------------

    fn assert_deterministic<G: Generator>(g: &G) {
        let mut r1 = rng(42);
        let mut r2 = rng(42);
        let seq1: Vec<Value> = (0..20).map(|_| g.generate(&mut r1)).collect();
        let seq2: Vec<Value> = (0..20).map(|_| g.generate(&mut r2)).collect();
        assert_eq!(seq1, seq2, "{} is not deterministic", g.name());
    }

    #[test]
    fn test_email_is_deterministic() {
        assert_deterministic(&EmailGenerator);
    }

    #[test]
    fn test_first_name_is_deterministic() {
        assert_deterministic(&FirstNameGenerator);
    }

    #[test]
    fn test_last_name_is_deterministic() {
        assert_deterministic(&LastNameGenerator);
    }

    #[test]
    fn test_full_name_is_deterministic() {
        assert_deterministic(&FullNameGenerator);
    }

    #[test]
    fn test_username_is_deterministic() {
        assert_deterministic(&UsernameGenerator);
    }

    #[test]
    fn test_phone_is_deterministic() {
        assert_deterministic(&PhoneGenerator);
    }

    #[test]
    fn test_url_is_deterministic() {
        assert_deterministic(&UrlGenerator);
    }

    #[test]
    fn test_password_is_deterministic() {
        assert_deterministic(&PasswordGenerator);
    }

    #[test]
    fn test_slug_is_deterministic() {
        assert_deterministic(&SlugGenerator);
    }

    #[test]
    fn test_sentence_is_deterministic() {
        assert_deterministic(&SentenceGenerator);
    }

    #[test]
    fn test_paragraph_is_deterministic() {
        assert_deterministic(&ParagraphGenerator);
    }

    #[test]
    fn test_company_name_is_deterministic() {
        assert_deterministic(&CompanyNameGenerator);
    }

    #[test]
    fn test_different_seeds_produce_different_output() {
        let mut r1 = rng(1);
        let mut r2 = rng(2);
        let v1 = EmailGenerator.generate(&mut r1);
        let v2 = EmailGenerator.generate(&mut r2);
        assert_ne!(v1, v2);
    }

    // --- format checks ------------------------------------------------------

    #[test]
    fn test_email_format() {
        let mut r = rng(1);
        let email = s(EmailGenerator.generate(&mut r));
        let parts: Vec<&str> = email.split('@').collect();
        assert_eq!(parts.len(), 2, "expected exactly one '@', got {email:?}");
        assert!(parts[0].contains('.'), "local part should contain '.'");
        assert!(parts[1].contains('.'), "domain should contain TLD");
        assert!(
            email == email.to_ascii_lowercase(),
            "email should be lowercase"
        );
    }

    #[test]
    fn test_first_name_is_capitalized() {
        let mut r = rng(1);
        let name = s(FirstNameGenerator.generate(&mut r));
        let first_char = name.chars().next().unwrap();
        assert!(first_char.is_ascii_uppercase(), "got {name:?}");
    }

    #[test]
    fn test_full_name_has_space() {
        let mut r = rng(1);
        let name = s(FullNameGenerator.generate(&mut r));
        assert!(name.contains(' '), "got {name:?}");
        assert_eq!(name.split_whitespace().count(), 2);
    }

    #[test]
    fn test_username_ends_with_digits() {
        let mut r = rng(1);
        let u = s(UsernameGenerator.generate(&mut r));
        assert!(u.chars().last().unwrap().is_ascii_digit(), "got {u:?}");
        assert!(u.chars().next().unwrap().is_ascii_lowercase());
    }

    #[test]
    fn test_phone_format() {
        let mut r = rng(1);
        let p = s(PhoneGenerator.generate(&mut r));
        assert!(p.starts_with("+1-"), "got {p:?}");
        let parts: Vec<&str> = p.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1].len(), 3);
        assert_eq!(parts[2].len(), 3);
        assert_eq!(parts[3].len(), 4);
    }

    #[test]
    fn test_url_starts_with_https() {
        let mut r = rng(1);
        let u = s(UrlGenerator.generate(&mut r));
        assert!(u.starts_with("https://"), "got {u:?}");
    }

    #[test]
    fn test_password_looks_like_bcrypt() {
        let mut r = rng(1);
        let p = s(PasswordGenerator.generate(&mut r));
        assert!(p.starts_with("$2b$10$"), "got {p:?}");
        // Total length: 7 ("$2b$10$") + 22 + 31 = 60
        assert_eq!(p.len(), 60, "got {p:?}");
    }

    #[test]
    fn test_slug_has_hyphens() {
        let mut r = rng(1);
        let sl = s(SlugGenerator.generate(&mut r));
        assert_eq!(sl.matches('-').count(), 2, "got {sl:?}");
        assert!(sl == sl.to_ascii_lowercase());
    }

    #[test]
    fn test_sentence_ends_with_period() {
        let mut r = rng(1);
        let sent = s(SentenceGenerator.generate(&mut r));
        assert!(sent.ends_with('.'), "got {sent:?}");
        let word_count = sent.trim_end_matches('.').split_whitespace().count();
        assert!(
            (5..=12).contains(&word_count),
            "{word_count} words: {sent:?}"
        );
    }

    #[test]
    fn test_paragraph_has_2_to_5_sentences() {
        let mut r = rng(1);
        let p = s(ParagraphGenerator.generate(&mut r));
        let n = p.matches('.').count();
        assert!((2..=5).contains(&n), "{n} sentences: {p:?}");
    }

    #[test]
    fn test_company_name_has_suffix() {
        let mut r = rng(1);
        let c = s(CompanyNameGenerator.generate(&mut r));
        let suffix = c.rsplit(' ').next().unwrap();
        assert!(
            COMPANY_SUFFIXES.contains(&suffix),
            "suffix {suffix:?} not in pool; full name {c:?}"
        );
    }

    // --- cross-generator determinism ----------------------------------------

    #[test]
    fn test_mixed_generator_sequence_is_deterministic() {
        let gens: Vec<Box<dyn Generator>> = vec![
            Box::new(EmailGenerator),
            Box::new(FirstNameGenerator),
            Box::new(UsernameGenerator),
            Box::new(PhoneGenerator),
            Box::new(PasswordGenerator),
            Box::new(SentenceGenerator),
        ];

        let run = || -> Vec<Value> {
            let mut r = rng(0xDEAD_BEEF);
            (0..6).map(|i| gens[i].generate(&mut r)).collect()
        };
        assert_eq!(run(), run());
    }
}
