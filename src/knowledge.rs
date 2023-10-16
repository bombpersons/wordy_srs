use std::{collections::{HashSet, HashMap}, str::FromStr, future::Future};

use log::info;
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, ConnectOptions, SqliteConnection, Pool, Sqlite, Row};
use lindera::tokenizer::Tokenizer;
use chrono::{Utc, Duration, FixedOffset, Local, Timelike, format::Fixed, DateTime};
use futures::TryStreamExt;

// https://supermemo.guru/wiki/SuperMemo_1.0_for_DOS_(1987)#Algorithm_SM-2
#[derive(Debug)]
struct SuperMemoItem {
    repitition: u32,
    duration: Duration,
    e_factor: f64
}

impl Default for SuperMemoItem {
    fn default() -> Self {
        SuperMemoItem { 
            repitition: 0,
            duration: Duration::zero(),
            e_factor: 2.5
        }
    }
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

    // This may be a little confusing, but this function returns the words rank in the frequency list.
    // Infrequent words will have higher values and frequent words will have lower values.
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

pub struct SentenceData {
    // The sentence and word that we are reviewing.
    pub sentence_text: String,
    pub sentence_id: i64,
    pub word_text: String,
    pub word_id: i64
}

pub struct IPlusOneSentenceData {
    pub sentence_text: String,
    pub sentence_id: i64,
    pub sentence_source: String,
    pub words_being_reviewed: Vec<(i64, String)>,
    pub words_that_are_new: Vec<(i64, String)>
}

pub struct ReviewInfoData {
    pub reviews_remaining: i64
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

    fn get_end_of_day_time(&self) -> DateTime<FixedOffset> {
        // Attempt to retrieve the word that is to be reviewed next.
        let now_time = Local::now().fixed_offset();

        // Calculate the end of the day (assuming 4am to be the end of the day)
        let day_end_hour = 4;
        if now_time.hour() < day_end_hour {
            now_time.clone().with_hour(day_end_hour)
        } else {
            (now_time + Duration::days(1)).with_hour(day_end_hour)
        }.unwrap() // TODO: error handling.
    }

    // Get a vector containing a tuple of word id and word text for all the words in a sentence.
    async fn get_words_in_sentence(&self, sentence_id: i64) -> Vec<(i64, String)> {
        let mut words = sqlx::query("
            SELECT word_id, sentence_id, words.text as word_text
            FROM word_sentence
                INNER JOIN words ON words.id = word_id
            WHERE sentence_id = ?")
            .bind(sentence_id)
            .fetch(&self.connection);

        let mut word_vec = Vec::new();
        while let Some(word_row) = words.try_next().await.unwrap() { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id").unwrap(), word_row.try_get("word_text").unwrap()));
        }

        word_vec
    }

    async fn get_words_in_sentence_that_need_reviewing(&self, sentence_id: i64) -> Vec<(i64, String)> {
        // First bit of useful info is how many reviews there are for today.
        let end_of_day_time = self.get_end_of_day_time();
        let now_time = Local::now().fixed_offset();

        let mut words = sqlx::query("
            SELECT word_id, sentence_id, words.text as word_text, words.next_review_at
            FROM word_sentence
                INNER JOIN words ON words.id = word_id
            WHERE sentence_id = ?
                AND (
                    reviewed = TRUE
                    AND datetime(next_review_at) < datetime(?) AND review_duration >= 86400
                    OR datetime(next_review_at) < datetime(?)
                )")
            .bind(sentence_id)
            .bind(end_of_day_time.to_rfc3339())
            .bind(now_time.to_rfc3339())
            .fetch(&self.connection);

        let mut word_vec = Vec::new();
        while let Some(word_row) = words.try_next().await.unwrap() { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id").unwrap(), word_row.try_get("word_text").unwrap()));
        }

        word_vec
    }

    async fn get_words_in_sentence_that_are_new(&self, sentence_id: i64) -> Vec<(i64, String)> {
        let mut words = sqlx::query("
            SELECT word_id, sentence_id, words.text as word_text, words.next_review_at
            FROM word_sentence
                INNER JOIN words ON words.id = word_id
            WHERE sentence_id = ?
                AND reviewed = FALSE")
            .bind(sentence_id)
            .fetch(&self.connection);

        let mut word_vec = Vec::new();
        while let Some(word_row) = words.try_next().await.unwrap() { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id").unwrap(), word_row.try_get("word_text").unwrap()));
        }

        word_vec
    }

    // [[ This query kind of finds sentences with fewest new words in them. ]]
    // SELECT word_id, sentence_id, sentences.text AS sentence_text, sentences.id, words.text AS word_text, words.frequency as word_frequency, words.reviewed as word_reviewed, AVG(words.frequency), COUNT(words.frequency)
    // FROM word_sentence
    //     INNER JOIN sentences ON sentences.id = sentence_id
    //     INNER JOIN words ON words.id = word_id
    // WHERE 
    //     word_reviewed = FALSE
    // GROUP BY
    //     sentence_id
    // ORDER by
    //     COUNT(words.frequency)
    
    // [[ This query finds a sentence that contains the most words that need to be reviewed now. ]]
    // SELECT word_id, sentence_id, sentences.text AS sentence_text, sentences.id, words.next_review_at as review_at, AVG(words.frequency), COUNT(words.frequency)
    // FROM word_sentence
    //     INNER JOIN sentences ON sentences.id = sentence_id
    //     INNER JOIN words ON words.id = word_id
    // WHERE 
    //     DATETIME(review_at) < DATETIME("2023-10-12T18:42:45+01:00")
    // GROUP BY
    //     sentence_id
    // ORDER BY
    //     COUNT(words.frequency) DESC
    // LIMIT 1

    // [[ This query finds sentences that contain the most words that need reviewing with the least amount of unknown words. ]]
    // SELECT 
    //     word_id, sentence_id, 
    //     sentences.text AS sentence_text, sentences.id, 
    //     words.next_review_at as review_at, words.reviewed AS reviewed, 
    //     SUM(CASE WHEN datetime(words.next_review_at) < datetime("2023-10-12T18:42:45+01:00") THEN 1 ELSE 0 END) as words_that_need_reviewing,
    //     SUM(CASE WHEN words.reviewed = FALSE THEN 1 ELSE 0 END) as words_that_are_new
    // FROM word_sentence
    //     INNER JOIN sentences ON sentences.id = sentence_id
    //     INNER JOIN words ON words.id = word_id
    // GROUP BY
    //     sentence_id
    // ORDER BY
    //     words_that_need_reviewing DESC,
    //     words_that_are_new ASC

    pub async fn get_next_sentence_i_plus_one(&self) -> IPlusOneSentenceData {
        let end_of_day_time = self.get_end_of_day_time();
        let now_time = Local::now().fixed_offset();

        info!("Attempting to find a sentence to review...");

        // First we need to find sentences that are most optimal to meet the criteria of reviewing words that are expired.
        // Treat a word as needing reviewing if it TODO: explain
        let review_sentence_row = sqlx::query("
            SELECT 
                word_id, sentence_id, 
                sentences.text AS sentence_text, sentences.id, sentences.source,
                words.next_review_at as review_at, words.reviewed AS reviewed, 
                SUM(CASE WHEN datetime(words.next_review_at) < datetime(?) AND review_duration >= 86400 OR datetime(words.next_review_at) < datetime(?) THEN 1 ELSE 0 END) as words_that_need_reviewing,
                SUM(CASE WHEN words.reviewed = FALSE THEN 1 ELSE 0 END) as words_that_are_new
            FROM word_sentence
                INNER JOIN sentences ON sentences.id = sentence_id
                INNER JOIN words ON words.id = word_id
            GROUP BY
                sentence_id
            ORDER BY
                words_that_need_reviewing DESC,
                words_that_are_new ASC
            LIMIT 1
            ")
            .bind(end_of_day_time.to_rfc3339())
            .bind(now_time.to_rfc3339())
            .fetch_one(&self.connection)
            .await.unwrap(); // TODO: error handling.

        // If there are no words that need reviewing in the selected sentence then we don't have any sentences to review!
        let words_that_need_reviewing: i64 = review_sentence_row.try_get("words_that_need_reviewing").unwrap();
        let words_that_are_new: i64 = review_sentence_row.try_get("words_that_are_new").unwrap();
        info!("Found a sentence with {} words that need reviewing and {} new words.", words_that_need_reviewing, words_that_are_new);

        // If there are words that need reviewing, do that!
        if words_that_need_reviewing > 0 {
            // If we review this sentence we'll be reviewing some of the words we need to review. Return it!
            let sentence_id = review_sentence_row.try_get("sentence_id").unwrap();
            let sentence_text = review_sentence_row.try_get("sentence_text").unwrap();
            let words_being_reviewed = self.get_words_in_sentence_that_need_reviewing(sentence_id).await;
            let words_that_are_new = self.get_words_in_sentence_that_are_new(sentence_id).await;
            let sentence_source = review_sentence_row.try_get("source").unwrap();

            return IPlusOneSentenceData {
                sentence_id,
                sentence_text,
                sentence_source,
                words_being_reviewed,
                words_that_are_new
            };
        }

        // Okay so there aren't any sentences that contain words that we need to review. 
        // Let's look for sentences that contain the least amount of new information so that we can learn new words.
        match sqlx::query("
            SELECT 
                word_id, sentence_id, 
                sentences.text AS sentence_text, sentences.id, sentences.source,
                words.reviewed as word_reviewed, 
                SUM(CASE WHEN words.reviewed = FALSE THEN 1 ELSE 0 END) as words_that_are_new,
                AVG(CASE WHEN words.reviewed = FALSE THEN words.count ELSE NULL END) as average_new_word_count
            FROM word_sentence
                INNER JOIN sentences ON sentences.id = sentence_id
                INNER JOIN words ON words.id = word_id
            GROUP BY
                sentence_id
            HAVING
                words_that_are_new > 0
            ORDER by
                words_that_are_new ASC,
                average_new_word_count DESC
            LIMIT 1")
            .fetch_one(&self.connection)
            .await {

            Ok(row) => {
                let words_that_are_new: i64 = row.try_get("words_that_are_new").unwrap();
                let averag_word_count: f64 = row.try_get("average_new_word_count").unwrap();
                info!("Found a sentence with {} new words with an average {} word count", words_that_are_new, averag_word_count);

                let sentence_id = row.try_get("sentence_id").unwrap();
                let sentence_text = row.try_get("sentence_text").unwrap();
                let words_being_reviewed = self.get_words_in_sentence_that_need_reviewing(sentence_id).await;
                let words_that_are_new = self.get_words_in_sentence_that_are_new(sentence_id).await;
                let sentence_source = row.try_get("source").unwrap();

                IPlusOneSentenceData {
                    sentence_id,
                    sentence_text,
                    sentence_source,
                    words_being_reviewed,
                    words_that_are_new
                }
            },

            Err(e) => {
                log::error!("{}", e);

                IPlusOneSentenceData {
                    sentence_id: 0,
                    sentence_text: "No sentence with any new words and no words are scheduled for reviewing.".to_string(),
                    sentence_source: "".to_string(),
                    words_being_reviewed: vec![(0, "".to_string())],
                    words_that_are_new: vec![(0, "".to_string())]
                }
            }
        }
    }

    pub async fn review_sentence(&self, sentence_id: i64, response_quality: f64) {
        // Find all the words in the sentence and then review them all!
        let words = self.get_words_in_sentence(sentence_id).await;
        for (word_id, word_text) in words {
            self.review_word(word_id, response_quality).await;
        }
    }

    pub async fn get_review_info(&self) -> ReviewInfoData {
        // First bit of useful info is how many reviews there are for today.
        let end_of_day_time = self.get_end_of_day_time();
        let now_time = Local::now().fixed_offset();

        let review_count: i64 = sqlx::query("
            SELECT COUNT(*) FROM words
            WHERE reviewed = TRUE
                AND datetime(next_review_at) < datetime(?) AND review_duration >= 86400
                OR datetime(next_review_at) < datetime(?)")
            .bind(end_of_day_time.to_rfc3339())
            .bind(now_time.to_rfc3339())
            .fetch_one(&self.connection).await.unwrap() // TODO: error handling.
            .try_get(0).unwrap();
        

        ReviewInfoData {
            reviews_remaining: review_count
        }
    }

    pub async fn review_word(&self, review_word_id: i64, response_quality: f64) {
        // First bit of useful info is how many reviews there are for today.
        let end_of_day_time = self.get_end_of_day_time();
        let now_time = Local::now().fixed_offset();

        // Get data related to the supermemo algorithm from the database.
        match sqlx::query("
            SELECT id, text, repitition, e_factor, review_duration, next_review_at, reviewed
            FROM words
                WHERE id = ?
                    AND (datetime(next_review_at) < datetime(?) AND review_duration >= 86400
                        OR datetime(next_review_at) < datetime(?)
                        OR reviewed = FALSE)")
            .bind(review_word_id)
            .bind(end_of_day_time.to_rfc3339())
            .bind(now_time.to_rfc3339())
            .fetch_one(&self.connection).await {

            Ok(word_row) => {
                // We found the word and it is a word that needs reviewing, or is a new word, so review it.
                
                // If this is a new word, use the default supermemo item.
                let reviewed: bool = word_row.try_get("reviewed").unwrap();
                let mut sm = if !reviewed { 
                    SuperMemoItem::default()
                } else {
                    SuperMemoItem {
                        repitition: word_row.try_get("repitition").unwrap(),
                        e_factor: word_row.try_get("e_factor").unwrap(),
                        duration: Duration::seconds(word_row.try_get("review_duration").unwrap())
                    }
                };

                // Calculate the values for the next review.
                sm = super_memo_2(sm, response_quality);
                let next_review_at = (Local::now().fixed_offset() + sm.duration).to_rfc3339();
        
                info!("Reviewing word id {}, updated review data: {:?}", review_word_id, &sm);
        
                // Store it.
                {
                    let mut tx = self.connection.begin().await.unwrap();
                    sqlx::query("
                        UPDATE words
                        SET repitition = ?,
                            e_factor = ?,
                            review_duration = ?,
                            next_review_at = ?,
                            reviewed = TRUE,
                            date_first_reviewed = CASE WHEN date_first_reviewed IS NULL THEN ? ELSE date_first_reviewed END
                        WHERE 
                            id = ?")
                        .bind(sm.repitition)
                        .bind(sm.e_factor)
                        .bind(sm.duration.num_seconds())
                        .bind(next_review_at)
                        .bind(now_time.to_rfc3339())
                        .bind(review_word_id)
                        .execute(&mut *tx).await.unwrap(); // TODO: error handling
        
                    tx.commit().await.unwrap(); // TODO: error handling
                }

            },
            Err(e) => {
                // There was an error (or there wasn't any word that matched.)
                // This is hopefully because the word doesn't need reviewing yet.
                // In this case we want to nothing other than log it.
                info!("Word with id {} doesn't need reviewing yet!", review_word_id);
            }
        }
    }

    async fn add_sentence(&self, sentence: &str, source: &str) {
        info!("Adding sentence {} from source {}", sentence, source);

        // Get the current datetime
        let now_time = Local::now().fixed_offset();

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
            "INSERT OR IGNORE INTO sentences(text, date_added, source)
                    VALUES(?, ?, ?)
                    RETURNING id;")
                .bind(sentence)
                .bind(now_time.to_rfc3339())
                .bind(source)
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
                    r#"INSERT INTO words(count, frequency, text, date_added)
                            VALUES(1, ?, ?, ?)
                            ON CONFLICT(text) DO UPDATE SET count=count + 1;"#)
                        .bind(freq)
                        .bind(&word)
                        .bind(now_time.to_rfc3339())
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

    pub async fn add_text(&self, text: &str, source: &str) {
        let sentences = iterate_sentences(text);
        for sentence in sentences {
            // Split the sentence into words and add that to the database.
            self.add_sentence(sentence.as_str(), source).await;
        }
    }
}