/// This is a direct port of the Python implementation of ABogus.
/// https://github.com/Johnserf-Seed/f2/blob/main/f2/utils/abogus.py
/// Original Python code is licensed under the Apache License 2.0
/// Credits to Johnserf-Seed
use libsm::sm3::hash::Sm3Hash;
use rand::RngExt;
use rand::seq::IndexedRandom;
use std::collections::HashMap;

use crate::extractor::default::DEFAULT_UA;

pub struct StringProcessor;

impl StringProcessor {
    /// Converts a slice of bytes into a String.
    #[inline]
    pub fn to_char_str(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| b as char).collect()
    }

    /// Converts a string into a vector of its byte values.
    #[inline]
    pub fn to_char_array(s: &str) -> Vec<u8> {
        s.bytes().collect()
    }

    /// Generates a pseudo-random byte string to obfuscate data.
    pub fn generate_random_bytes(length: usize) -> String {
        let mut rng = rand::rng();
        let mut result = Vec::new();

        let mut generate_byte_sequence = || -> Vec<u8> {
            let rd = rng.random_range(0..10000) as u64;
            vec![
                (((rd & 255) & 170) | 1) as u8,
                (((rd & 255) & 85) | 2) as u8,
                (((rd >> 8) & 170) | 5) as u8,
                (((rd >> 8) & 85) | 40) as u8,
            ]
        };

        for _ in 0..length {
            result.extend(generate_byte_sequence());
        }

        Self::to_char_str(&result)
    }
}

pub struct CryptoUtility {
    salt: String,
    base64_alphabet: Vec<Vec<char>>,
    big_array: Vec<u8>,
}

impl CryptoUtility {
    pub fn new(salt: &str, custom_base64_alphabet: Vec<&str>) -> Self {
        let base64_alphabet = custom_base64_alphabet
            .into_iter()
            .map(|s| s.chars().collect())
            .collect();

        let big_array = vec![
            121, 243, 55, 234, 103, 36, 47, 228, 30, 231, 106, 6, 115, 95, 78, 101, 250, 207, 198,
            50, 139, 227, 220, 105, 97, 143, 34, 28, 194, 215, 18, 100, 159, 160, 43, 8, 169, 217,
            180, 120, 247, 45, 90, 11, 27, 197, 46, 3, 84, 72, 5, 68, 62, 56, 221, 75, 144, 79, 73,
            161, 178, 81, 64, 187, 134, 117, 186, 118, 16, 241, 130, 71, 89, 147, 122, 129, 65, 40,
            88, 150, 110, 219, 199, 255, 181, 254, 48, 4, 195, 248, 208, 32, 116, 167, 69, 201, 17,
            124, 125, 104, 96, 83, 80, 127, 236, 108, 154, 126, 204, 15, 20, 135, 112, 158, 13, 1,
            188, 164, 210, 237, 222, 98, 212, 77, 253, 42, 170, 202, 26, 22, 29, 182, 251, 10, 173,
            152, 58, 138, 54, 141, 185, 33, 157, 31, 252, 132, 233, 235, 102, 196, 191, 223, 240,
            148, 39, 123, 92, 82, 128, 109, 57, 24, 38, 113, 209, 245, 2, 119, 153, 229, 189, 214,
            230, 174, 232, 63, 52, 205, 86, 140, 66, 175, 111, 171, 246, 133, 238, 193, 99, 60, 74,
            91, 225, 51, 76, 37, 145, 211, 166, 151, 213, 206, 0, 200, 244, 176, 218, 44, 184, 172,
            49, 216, 93, 168, 53, 21, 183, 41, 67, 85, 224, 155, 226, 242, 87, 177, 146, 70, 190,
            12, 162, 19, 137, 114, 25, 165, 163, 192, 23, 59, 9, 94, 179, 107, 35, 7, 142, 131,
            239, 203, 149, 136, 61, 249, 14, 156,
        ];

        CryptoUtility {
            salt: salt.to_string(),
            base64_alphabet,
            big_array,
        }
    }

    /// Computes the SM3 hash of the input data.
    pub fn sm3_to_array(input_data: &[u8]) -> Vec<u8> {
        Sm3Hash::new(input_data).get_hash().to_vec()
    }

    /// Adds salt to a string parameter.
    pub fn add_salt(&self, param: &str) -> String {
        format!("{}{}", param, self.salt)
    }

    /// Hashes a string parameter, optionally adding salt first.
    pub fn params_to_array(&self, param: &str, add_salt: bool) -> Vec<u8> {
        let processed_param = if add_salt {
            self.add_salt(param)
        } else {
            param.to_string()
        };
        Self::sm3_to_array(processed_param.as_bytes())
    }

    /// Encrypts/decrypts the input byte list. This method is stateful and modifies `big_array`.
    pub fn transform_bytes(&mut self, values_list: &[u32]) -> Vec<u32> {
        let mut result_vec = Vec::with_capacity(values_list.len());
        let mut index_b = self.big_array[1] as usize;
        let mut initial_value: u8 = 0;
        let mut value_e: u8 = 0;
        let array_len = self.big_array.len();

        for (index, &char_code) in values_list.iter().enumerate() {
            let sum_initial = if index == 0 {
                initial_value = self.big_array[index_b];
                let sum_val = (index_b as u8).wrapping_add(initial_value);
                self.big_array[1] = initial_value;
                self.big_array[index_b] = index_b as u8;
                sum_val
            } else {
                initial_value.wrapping_add(value_e)
            };

            let sum_initial_idx = (sum_initial as usize) % array_len;
            let value_f = self.big_array[sum_initial_idx];

            // Perform the XOR operation on the full u32 value.
            result_vec.push(char_code ^ (value_f as u32));

            let next_idx = (index + 2) % array_len;
            value_e = self.big_array[next_idx];
            let new_sum_initial_idx = ((index_b as u8).wrapping_add(value_e) as usize) % array_len;
            initial_value = self.big_array[new_sum_initial_idx];

            self.big_array.swap(new_sum_initial_idx, next_idx);
            index_b = new_sum_initial_idx;
        }

        result_vec
    }

    /// Encodes a string using a custom Base64 alphabet.
    pub fn base64_encode(&self, bytes: &[u8], selected_alphabet: usize) -> String {
        let alphabet = &self.base64_alphabet[selected_alphabet];

        // Pre-allocate output string with estimated capacity
        let mut output_string = String::with_capacity((bytes.len() * 4).div_ceil(3));

        // Process bytes in chunks of 3 to avoid string operations
        for chunk in bytes.chunks(3) {
            let b1 = chunk[0];
            let b2 = chunk.get(1).copied().unwrap_or(0);
            let b3 = chunk.get(2).copied().unwrap_or(0);

            // Combine 3 bytes into a 24-bit number
            let combined = ((b1 as u32) << 16) | ((b2 as u32) << 8) | (b3 as u32);

            // Extract four 6-bit values and map to alphabet
            output_string.push(alphabet[((combined >> 18) & 63) as usize]);
            output_string.push(alphabet[((combined >> 12) & 63) as usize]);

            if chunk.len() > 1 {
                output_string.push(alphabet[((combined >> 6) & 63) as usize]);
            }
            if chunk.len() > 2 {
                output_string.push(alphabet[(combined & 63) as usize]);
            }
        }

        // Add padding
        let padding_needed = (4 - output_string.len() % 4) % 4;
        if padding_needed > 0 {
            output_string.push_str(&"=".repeat(padding_needed));
        }

        output_string
    }

    /// Encodes a byte string using a custom Base64-like scheme with shifts and padding.
    pub fn abogus_encode(&self, values: &[u32], selected_alphabet: usize) -> String {
        let alphabet = &self.base64_alphabet[selected_alphabet];
        let len = values.len();

        let mut abogus = String::with_capacity((len * 4).div_ceil(3));

        for chunk in values.chunks(3) {
            let v1 = chunk[0];
            let v2 = chunk.get(1).copied().unwrap_or(0);
            let v3 = chunk.get(2).copied().unwrap_or(0);

            let n = (v1 << 16) | (v2 << 8) | v3;

            abogus.push(alphabet[((n & 0xFC0000) >> 18) as usize]);
            abogus.push(alphabet[((n & 0x03F000) >> 12) as usize]);

            if chunk.len() > 1 {
                abogus.push(alphabet[((n & 0x0FC0) >> 6) as usize]);
            }
            if chunk.len() > 2 {
                abogus.push(alphabet[(n & 0x3F) as usize]);
            }
        }

        let padding = (4 - abogus.len() % 4) % 4;
        if padding > 0 {
            abogus.push_str(&"=".repeat(padding));
        }
        abogus
    }

    /// Encrypts data using the RC4 algorithm.
    pub fn rc4_encrypt(key: &[u8], plaintext: &str) -> Vec<u8> {
        let mut s: [u8; 256] = [0; 256];
        for (i, elem) in s.iter_mut().enumerate() {
            *elem = i as u8;
        }

        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }

        let mut i: u8 = 0;
        let mut j: u8 = 0;
        let plaintext_bytes = plaintext.as_bytes();
        let mut ciphertext = Vec::with_capacity(plaintext_bytes.len());

        for &char_val in plaintext_bytes {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            let k = s[s[i as usize].wrapping_add(s[j as usize]) as usize];
            ciphertext.push(char_val ^ k);
        }
        ciphertext
    }
}

pub struct BrowserFingerprintGenerator;

impl BrowserFingerprintGenerator {
    pub fn generate_fingerprint(browser_type: &str) -> String {
        match browser_type {
            "Chrome" => Self::generate_chrome_fingerprint(),
            "Firefox" => Self::generate_firefox_fingerprint(),
            "Safari" => Self::generate_safari_fingerprint(),
            "Edge" => Self::generate_edge_fingerprint(),
            _ => Self::generate_chrome_fingerprint(), // Default
        }
    }

    pub fn generate_chrome_fingerprint() -> String {
        Self::_generate_fingerprint("Win32")
    }

    pub fn generate_firefox_fingerprint() -> String {
        Self::_generate_fingerprint("Win32")
    }

    pub fn generate_safari_fingerprint() -> String {
        Self::_generate_fingerprint("MacIntel")
    }

    pub fn generate_edge_fingerprint() -> String {
        Self::_generate_fingerprint("Win32")
    }

    fn _generate_fingerprint(platform: &str) -> String {
        let mut rng = rand::rng();
        let inner_width = rng.random_range(1024..=1920);
        let inner_height = rng.random_range(768..=1080);
        let outer_width = inner_width + rng.random_range(24..=32);
        let outer_height = inner_height + rng.random_range(75..=90);
        let screen_x = 0;
        let screen_y = [0, 30].choose(&mut rng).copied().unwrap_or(0);
        let size_width = rng.random_range(1024..=1920);
        let size_height = rng.random_range(768..=1080);
        let avail_width = rng.random_range(1280..=1920);
        let avail_height = rng.random_range(800..=1080);

        format!(
            "{inner_width}|{inner_height}|{outer_width}|{outer_height}|{screen_x}|{screen_y}|0|0|{size_width}|{size_height}|{avail_width}|{avail_height}|{inner_width}|{inner_height}|24|24|{platform}",
        )
    }
}

pub struct ABogus {
    crypto_utility: CryptoUtility,
    user_agent: String,
    browser_fp: String,
    options: Vec<u64>,
    page_id: u64,
    aid: u64,
    ua_key: Vec<u8>,
    sort_index: Vec<u8>,
    sort_index_2: Vec<u8>,
}

impl ABogus {
    /// Creates a new `ABogus` instance.
    /// `fp` and `user_agent` are optional; if `None`, defaults will be used.
    pub fn new(fp: Option<&str>, user_agent: Option<&str>, options: Option<Vec<u64>>) -> Self {
        let salt = "cus";
        let character = "Dkdpgh2ZmsQB80/MfvV36XI1R45-WUAlEixNLwoqYTOPuzKFjJnry79HbGcaStCe";
        let character2 = "ckdp1h4ZKsUB80/Mfvw36XIgR25+WQAlEi7NLboqYTOPuzmFjJnryx9HVGDaStCe";
        let character_list = vec![character, character2];

        let crypto_utility = CryptoUtility::new(salt, character_list);

        let final_user_agent = user_agent
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_UA)
            .to_string();

        let final_fp = fp
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| BrowserFingerprintGenerator::generate_fingerprint("Edge"));

        let final_options = options.unwrap_or_else(|| vec![0, 1, 14]);

        ABogus {
            crypto_utility,
            user_agent: final_user_agent,
            browser_fp: final_fp,
            options: final_options,
            page_id: 0,
            aid: 6383,
            ua_key: vec![0x00, 0x01, 0x0E],
            sort_index: vec![
                18, 20, 52, 26, 30, 34, 58, 38, 40, 53, 42, 21, 27, 54, 55, 31, 35, 57, 39, 41, 43,
                22, 28, 32, 60, 36, 23, 29, 33, 37, 44, 45, 59, 46, 47, 48, 49, 50, 24, 25, 65, 66,
                70, 71,
            ],
            sort_index_2: vec![
                18, 20, 26, 30, 34, 38, 40, 42, 21, 27, 31, 35, 39, 41, 43, 22, 28, 32, 36, 23, 29,
                33, 37, 44, 45, 46, 47, 48, 49, 50, 24, 25, 52, 53, 54, 55, 57, 58, 59, 60, 65, 66,
                70, 71,
            ],
        }
    }

    /// Generates the A-Bogus parameter.
    /// Returns a tuple: `(full_params_string, bogus_value, user_agent, body)`.
    pub fn generate_abogus(
        &mut self,
        params: &str,
        body: &str,
    ) -> (String, String, String, String) {
        let mut ab_dir: HashMap<u8, u64> = HashMap::new();
        ab_dir.insert(8, 3);
        ab_dir.insert(18, 44);
        ab_dir.insert(66, 0);
        ab_dir.insert(69, 0);
        ab_dir.insert(70, 0);
        ab_dir.insert(71, 0);

        let start_encryption = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Hash(Hash(params))
        let params_hash_1 = self.crypto_utility.params_to_array(params, true);
        let array1 = CryptoUtility::sm3_to_array(&params_hash_1);

        // Hash(Hash(body))
        let body_hash_1 = self.crypto_utility.params_to_array(body, true);
        let array2 = CryptoUtility::sm3_to_array(&body_hash_1);

        // Hash(Base64(RC4(user_agent)))
        let rc4_ua = CryptoUtility::rc4_encrypt(&self.ua_key, &self.user_agent);

        let ua_b64 = self.crypto_utility.base64_encode(&rc4_ua, 1);
        let array3 = self.crypto_utility.params_to_array(&ua_b64, false);

        let end_encryption = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(start_encryption);

        // Populate ab_dir with dynamic values
        ab_dir.insert(20, (start_encryption >> 24) & 255);
        ab_dir.insert(21, (start_encryption >> 16) & 255);
        ab_dir.insert(22, (start_encryption >> 8) & 255);
        ab_dir.insert(23, start_encryption & 255);
        ab_dir.insert(24, start_encryption / 0x100000000);
        ab_dir.insert(25, start_encryption / 0x10000000000);

        ab_dir.insert(26, (self.options[0] >> 24) & 255);
        ab_dir.insert(27, (self.options[0] >> 16) & 255);
        ab_dir.insert(28, (self.options[0] >> 8) & 255);
        ab_dir.insert(29, self.options[0] & 255);

        ab_dir.insert(30, (self.options[1] / 256) & 255);
        ab_dir.insert(31, (self.options[1] % 256) & 255);
        ab_dir.insert(32, (self.options[1] >> 24) & 255);
        ab_dir.insert(33, (self.options[1] >> 16) & 255);

        ab_dir.insert(34, (self.options[2] >> 24) & 255);
        ab_dir.insert(35, (self.options[2] >> 16) & 255);
        ab_dir.insert(36, (self.options[2] >> 8) & 255);
        ab_dir.insert(37, self.options[2] & 255);

        ab_dir.insert(38, array1[21] as u64);
        ab_dir.insert(39, array1[22] as u64);
        ab_dir.insert(40, array2[21] as u64);
        ab_dir.insert(41, array2[22] as u64);
        ab_dir.insert(42, array3[23] as u64);
        ab_dir.insert(43, array3[24] as u64);

        ab_dir.insert(44, (end_encryption >> 24) & 255);
        ab_dir.insert(45, (end_encryption >> 16) & 255);
        ab_dir.insert(46, (end_encryption >> 8) & 255);
        ab_dir.insert(47, end_encryption & 255);
        // Key 8 is inserted above and should exist; fall back to 0 to avoid panic.
        ab_dir.insert(48, *ab_dir.get(&8).unwrap_or(&0));
        ab_dir.insert(49, end_encryption / 0x100000000);
        ab_dir.insert(50, end_encryption / 0x10000000000);

        ab_dir.insert(51, (self.page_id >> 24) & 255);
        ab_dir.insert(52, (self.page_id >> 16) & 255);
        ab_dir.insert(53, (self.page_id >> 8) & 255);
        ab_dir.insert(54, self.page_id & 255);
        ab_dir.insert(55, self.page_id);
        ab_dir.insert(56, self.aid);
        ab_dir.insert(57, self.aid & 255);
        ab_dir.insert(58, (self.aid >> 8) & 255);
        ab_dir.insert(59, (self.aid >> 16) & 255);
        ab_dir.insert(60, (self.aid >> 24) & 255);

        ab_dir.insert(64, self.browser_fp.len() as u64);
        ab_dir.insert(65, self.browser_fp.len() as u64);

        let mut sorted_values: Vec<u32> = self
            .sort_index
            .iter()
            .map(|&i| *ab_dir.get(&i).unwrap_or(&0) as u32)
            .collect();

        let fp_array = StringProcessor::to_char_array(&self.browser_fp);

        let mut ab_xor: u32 = 0;
        for (index, &key) in self.sort_index_2.iter().enumerate() {
            let val = *ab_dir.get(&key).unwrap_or(&0) as u32;
            if index == 0 {
                ab_xor = val;
            } else {
                ab_xor ^= val;
            }
        }

        sorted_values.extend(fp_array.iter().map(|&b| b as u32));
        sorted_values.push(ab_xor);

        let transformed_values: Vec<u32> = self.crypto_utility.transform_bytes(&sorted_values);
        let random_prefix: Vec<u32> = StringProcessor::generate_random_bytes(3)
            .chars()
            .map(|c| c as u32)
            .collect();

        let final_values: Vec<u32> = random_prefix
            .into_iter()
            .chain(transformed_values)
            .collect();

        let abogus = self.crypto_utility.abogus_encode(&final_values, 0);
        let final_params = format!("{params}&a_bogus={abogus}");

        (
            final_params,
            abogus,
            self.user_agent.clone(),
            body.to_string(),
        )
    }
}

#[cfg(test)]
mod test {
    use std::time::Instant;

    use crate::extractor::platforms::douyin::abogus::{ABogus, BrowserFingerprintGenerator};

    #[test]
    fn test_abogus() {
        let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
        let chrome_fp = BrowserFingerprintGenerator::generate_fingerprint("Edge");
        let mut abogus = ABogus::new(Some(&chrome_fp), Some(user_agent), None);

        println!("--- GET Example ---");
        let get_url = "https://www.douyin.com/aweme/v1/web/aweme/detail/?";
        let get_params = "device_platform=webapp&aid=6383&channel=channel_pc_web&sec_user_id=MS4wLjABAAAArDVBosPJF3eIWVEFp0szuJ-e1V_-rK0ieJeWwpE77E8&max_cursor=0&locate_query=false&show_live_replay_strategy=1&need_time_list=1&time_list_query=0&whale_cut_token=&cut_version=1&count=18&publish_video_strategy_type=2&from_user_page=1&update_version_code=170400&pc_client_type=1&pc_libra_divert=Windows&support_h265=1&support_dash=0&version_code=290100&version_name=29.1.0&cookie_enabled=true&screen_width=1920&screen_height=1080&browser_language=zh-CN&browser_platform=Win32&browser_name=Edge&browser_version=131.0.0.0&browser_online=true&engine_name=Blink&engine_version=131.0.0.0&os_name=Windows&os_version=10&cpu_core_num=12&device_memory=8&platform=PC&downlink=10&effective_type=4g&round_trip_time=50";
        let get_body = "";
        let (get_result_params, a_bogus, _, _) = abogus.generate_abogus(get_params, get_body);
        println!("{get_url}{get_result_params}");
        println!("a_bogus : {a_bogus}");
        println!();

        println!("--- POST Example ---");
        let post_url = "https://www.douyin.com/aweme/v2/web/aweme/stats/?";
        let post_params = "device_platform=webapp&aid=6383&channel=channel_pc_web&pc_client_type=1&pc_libra_divert=Windows&update_version_code=170400&support_h265=1&support_dash=0&version_code=170400&version_name=17.4.0&cookie_enabled=true&screen_width=1920&screen_height=1080&browser_language=zh-CN&browser_platform=Win32&browser_name=Edge&browser_version=131.0.0.0&browser_online=true&engine_name=Blink&engine_version=131.0.0.0&os_name=Windows&os_version=10&cpu_core_num=12&device_memory=8&platform=PC&downlink=10&effective_type=4g&round_trip_time=50";
        let post_body = "aweme_type=0&item_id=7467485482314763572&play_delta=1&source=0";
        let (post_result_params, _, _, _) = abogus.generate_abogus(post_params, post_body);
        println!("{post_url}{post_result_params}");
        println!();

        // Timing tests
        println!("--- Benchmarks ---");
        let start = Instant::now();
        for _ in 0..100 {
            abogus.generate_abogus(get_params, get_body);
        }
        println!("Generating 100 A-Bogus params took: {:?}", start.elapsed());

        let start = Instant::now();
        for _ in 0..100 {
            BrowserFingerprintGenerator::generate_fingerprint("Chrome");
        }
        println!("Generating 100 fingerprints took: {:?}", start.elapsed());
    }
}
