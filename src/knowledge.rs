use std::{collections::{HashSet, HashMap}, str::FromStr, future::Future};

use log::info;
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, ConnectOptions, SqliteConnection, Pool, Sqlite, Row};
use lindera::tokenizer::Tokenizer;
use chrono::{Utc, Duration};
use futures::TryStreamExt;

// https://supermemo.guru/wiki/SuperMemo_1.0_for_DOS_(1987)#Algorithm_SM-2
#[derive(Debug)]
struct SuperMemoItem {
    repitition: u32,
    duration: Duration,
    e_factor: f64
}

fn mul_duration(duration: Duration, multiplier: f64) -> Duration {
    let new_interval_secs = duration.num_seconds() as f64 * multiplier;
    Duration::seconds(new_interval_secs as i64)
}

fn super_memo_2(item: SuperMemoItem, response_quality: f64) -> SuperMemoItem {
    let repitition = if response_quality < 3.0 { 0 } else { item.repitition };

    match repitition {
        0 => SuperMemoItem {
             repitition: 1,
             duration: Duration::minutes(10),
             e_factor: item.e_factor
        },
        1 => SuperMemoItem {
            repitition: 2,
            duration: Duration::days(1),
            e_factor: item.e_factor
        },
        r => {
            let e_factor = (item.e_factor + (0.1 - (5.0 - response_quality) * (0.08 + (5.0 - response_quality) * 0.02))).max(1.3);
            let duration = mul_duration(item.duration, e_factor);
            let repitition = repitition + 1;

            SuperMemoItem {
                repitition,
                duration,
                e_factor
            }
        }
    }
}

// A lookup table for word frequency.
#[derive(Clone)]
struct WordFrequencyList {
    words: HashMap<String, i64>
}

impl WordFrequencyList {
    fn new() -> Self {
        let wordlist = include_str!("japanese_word_frequency.txt");
        let mut words = HashMap::new();
        for (index, line) in wordlist.lines().enumerate() {
            words.insert(line.to_string(), index as i64);
        }

        Self { 
            words
        }
    }

    fn get_word_freq(&self, word: &str) -> i64 {
        match self.words.get(word) {
            Some(freq) => *freq,
            None => self.words.len() as i64 // If it's not on the list if must be very infrequent
                                            // Treat it as though it's at the bottom of the list.
        }
    }
}

// Try and split up a text into sentences.
fn iterate_sentences(text: &str) -> Vec<String> {
    let terminators: HashSet<char> = HashSet::from(['。', '\n', '！', '？']);
    let open_quotes: HashSet<char> = HashSet::from(['「']);
    let close_quotes: HashSet<char> = HashSet::from(['」']);

    let mut depth: i32 = 0;
    let mut curr_string: String = String::new();
    let mut sentences = Vec::new();
    for c in text.chars() {
        curr_string.push(c);

        if open_quotes.contains(&c) {
            depth += 1;
        }
        else if close_quotes.contains(&c) {
            depth -= 1;
        }
        else if depth == 0 && terminators.contains(&c) {
            let sentence = curr_string.trim();

            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }

            curr_string.clear();
        }
    }
    sentences
}

#[derive(Clone)]
pub struct Knowledge {
    tokenizer: Tokenizer,
    word_freq: WordFrequencyList,
    connection: Pool<Sqlite>
}

impl Knowledge {
    pub async fn new() -> Self {
        // Create the dtabase.
        let connection = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::from_str("db.sqlite").unwrap() // TODO: error handling
                .create_if_missing(true)
            )
            .await.unwrap(); // TODO: error handling.

        sqlx::migrate!().run(&connection).await.unwrap(); // TODO: error handling.

        // A tokenizer to split up sentences into words.
        let tokenizer = Tokenizer::new().unwrap(); // TODO: error handling.

        Self {
            tokenizer,
            word_freq: WordFrequencyList::new(),
            connection
        }
    }

    async fn get_next_word(&self) -> Option<(String, i64)> {
        // Attempt to retrieve the word that is to be reviewed next.
        let now_time = format!("{}", Utc::now());
        let mut word_and_id: Option<(String, i64)> = match sqlx::query("
            SELECT repitition, next_review_at, text, id FROM words
            WHERE reviewd = TRUE
                AND next_review < ? 
            ORDER BY next_review at ASC
            LIMIT 1")
            .bind(now_time)
            .fetch_one(&self.connection)
            .await {
                
                Ok(row) => { 
                    Some((row.try_get("text").unwrap(), row.try_get("id").unwrap()))
                }, 
                Err(e) => {
                    info!("No scheduled word to review!");
                    None 
                }
        };

        // Find a word that hasn't been reviewed yet then. Use the highest frequency word available.
        if word_and_id.is_none() {
            word_and_id = match sqlx::query("
                SELECT id, text, frequency, reviewed FROM words
                WHERE reviewed = FALSE
                ORDER BY frequency ASC
                LIMIT 1")
                .fetch_one(&self.connection)
                .await {

                Ok(row) => {
                    Some((row.try_get("text").unwrap(), row.try_get("id").unwrap()))
                },
                Err(e) => {
                    info!("No new words to review!");
                    None
                }
            }
        }

        word_and_id
    }

    pub async fn get_next_sentence(&self) -> (String, String) {
        // Get the word we're supposed to be reviewing.
        let next_word_and_id = self.get_next_word().await;

        // We want to pick the sentence that has the least amount of new information to the user.
        // Iterate over the words in each sentence and calculate a heuristic based on the 
        // frequency of the words contained.
        let mut lowest_heuristic = i64::MAX;
        let mut fittest_sentence = None;

        // If a word was found...
        if let Some((next_word, next_word_id)) = next_word_and_id {

            // Now get a list of sentences that include this word.
            let mut sentences = sqlx::query("
                SELECT word_id, sentence_id, sentences.id, sentences.text
                FROM word_sentence
                    INNER JOIN sentences ON sentence_id = sentences.id
                WHERE word_id = ?")
                .bind(next_word_id)
                .fetch(&self.connection);

            // Iterate over them.
            while let Some(sentence_row) = sentences.try_next().await.unwrap() { // TODO: error handling.
                let sentence_id: i64 = sentence_row.try_get("sentence_id").unwrap(); // TODO: error handling.
                let sentence_text: String = sentence_row.try_get("text").unwrap(); // TODO: error handling.

                // Find all words in the sentence.
                let mut words = sqlx::query("
                    SELECT word_id, sentence_id, words.frequency, words.reviewed FROM word_sentence
                        INNER JOIN words ON word_id = words.id
                    WHERE sentence_id = ?")
                    .bind(sentence_id)
                    .fetch(&self.connection);

                // Calculate our heuristic for new information in the sentence.
                let mut heuristic = 0;

                // Iterate over the words.
                while let Some(word_row) = words.try_next().await.unwrap() { // TODO: error handling.
                    let word_id: i64 = word_row.try_get("word_id").unwrap(); // TODO: error handling.
                    let word_frequency: i64 = word_row.try_get("frequency").unwrap(); // TODO: error handling.
                    let word_reviewed_count: i64 = word_row.try_get("reviewed").unwrap(); //TODO: error handling.

                    // If this is the word we are currently reviewing or if we have reviewed it in the past (it's known)
                    // Then don't add to our heuristic.
                    if word_id != next_word_id || word_reviewed_count == 0 {
                        heuristic += word_frequency;
                    }
                }

                info!("Considering sentence with {} heuristic: {}", &heuristic, &sentence_text);

                // Is this lower than our lowest heuristic so far
                if heuristic < lowest_heuristic {
                    lowest_heuristic = heuristic;
                    fittest_sentence = Some((next_word.clone(), sentence_text));
                }
            }
        }

        match fittest_sentence {
            Some((word, sentence)) => { (word, sentence) },
            None => { ("".to_string(),"No sentence to review".to_string()) }
        }
    }

    pub async fn review_sentence(&self, response_quality: f64) {
        // Get the word we are reviewing.
        let reviewing_word_and_id = self.get_next_word().await;
        if let Some((_, reviewing_word_id)) = reviewing_word_and_id {

            // Get data related to the supermemo algorithm from the database.
            let word_row = sqlx::query("
                SELECT id, text, repitition, e_factor, review_duration
                FROM words
                    WHERE id = ?")
                .bind(reviewing_word_id)
                .fetch_one(&self.connection).await.unwrap(); // TODO: error handling

            let mut sm = SuperMemoItem {
                repitition: word_row.try_get("repitition").unwrap(),
                e_factor: word_row.try_get("e_factor").unwrap(),
                duration: Duration::seconds(word_row.try_get("review_duration").unwrap())
            };

            // Calculate the values for the next review.
            sm = super_memo_2(sm, response_quality);
            let next_review_at = format!("{}", Utc::now() + sm.duration);

            info!("Reviewing word id {}, updated review data: {:?}", reviewing_word_id, &sm);

            // Store it.
            {
                let mut tx = self.connection.begin().await.unwrap();
                sqlx::query("
                    UPDATE words
                    SET repitition = ?,
                        e_factor = ?,
                        review_duration = ?,
                        next_review_at = ?,
                        reviewed = TRUE
                    WHERE 
                        id = ?")
                    .bind(sm.repitition)
                    .bind(sm.e_factor)
                    .bind(sm.duration.num_seconds())
                    .bind(next_review_at)
                    .bind(reviewing_word_id)
                    .execute(&mut *tx).await.unwrap(); // TODO: error handling

                tx.commit().await.unwrap(); // TODO: error handling
            }
        }
    }

    async fn add_sentence(&self, sentence: &str) {
        info!("Adding sentence: {}", sentence);

        // Tokenize the sentence to get the words.
        let tokens = self.tokenizer.tokenize(sentence).unwrap();
        let mut words = Vec::<String>::new();
        for token in tokens {
            if token.detail.len() > 7 {
                let base_form = &token.detail[6];
                words.push(base_form.to_string());
            }
        }

        // Start a database transaction.
        let mut tx = self.connection.begin().await.unwrap(); // TODO: error handling

        // Insert the sentence to the sentences table.
        let sentence_id: Option<i64> = match sqlx::query(
            "INSERT OR IGNORE INTO sentences(text)
                    VALUES(?)
                    RETURNING id;")
                .bind(sentence)
                .fetch_one(&mut *tx).await {
                    
                Err(e) => None,
                Ok(row) => Some(row.try_get("id").expect("No id in inserted sentence."))
            };
        
        // If the sentence already existed, then we haven't done anything and we don't have a new sentence id.
        // The words will have already been inserted the first time we added the sentence.
        if let Some(sentence_id) = sentence_id  {
            // Let's go over the words.
            for word in words {
                let freq = self.word_freq.get_word_freq(&word);

                // Insert into known words, or increment count if we already have it.
                sqlx::query(
                    r#"INSERT INTO words(count, frequency, text)
                            VALUES(1, ?, ?)
                            ON CONFLICT(text) DO UPDATE SET count=count + 1;"#)
                        .bind(freq)
                        .bind(&word)
                        .execute(&mut *tx).await.expect("Error adding word!");

                // Create the word->sentence relationship.
                let word_id: i64 = sqlx::query(
                    r#"SELECT id, text
                            FROM words
                            WHERE text = ?"#)
                    .bind(&word)
                    .fetch_one(&mut *tx).await.expect("Couldn't find word in database!")
                    .try_get("id").expect("No id in word table");

                sqlx::query(
                    r#"INSERT OR IGNORE INTO word_sentence(word_id, sentence_id)
                            VALUES(?, ?);"#)
                    .bind(word_id)
                    .bind(sentence_id)
                    .execute(&mut *tx).await.expect("Couldn't add word->sentence relationship.");
            }
        }

        // Commit to the transaction.
        tx.commit().await.unwrap(); // TODO: error handling
    }

    pub async fn add_text(&self, text: &str) {
        let sentences = iterate_sentences(text);
        for sentence in sentences {
            // Split the sentence into words and add that to the database.
            self.add_sentence(sentence.as_str()).await;
        }
    }
}