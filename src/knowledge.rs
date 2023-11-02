use std::{collections::{HashSet, HashMap}, str::FromStr, future::Future, process::{Command, Stdio}, io::{Write}, string, fmt::Display};

use askama::Error;
use log::info;
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow}, ConnectOptions, SqliteConnection, Pool, Sqlite, Row, Transaction, Executor, SqliteExecutor, error::DatabaseError};
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
    let open_quotes: HashSet<char> = HashSet::from(['「', '『', '（']);
    let close_quotes: HashSet<char> = HashSet::from(['」', '』', '）']);

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

#[derive(Debug)]
pub enum KnowledgeError {
    DatabaseError(sqlx::Error),
    MigrationError(sqlx::migrate::MigrateError),
    TokenizeError
}

impl Display for KnowledgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DatabaseError(e) => write!(f, "Database error! Error: {}", e),
            Self::MigrationError(e) => write!(f, "Migration error! Error: {}", e),
            Self::TokenizeError => write!(f, "Error tokenizing sentence!")
        }
    }
}

impl std::error::Error for KnowledgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DatabaseError(e) => Some(e),
            Self::MigrationError(e) => Some(e),
            Self::TokenizeError => None
        }
    }
}

impl From<sqlx::Error> for KnowledgeError {
    fn from(value: sqlx::Error) -> Self {
        KnowledgeError::DatabaseError(value)
    }
}

impl From<sqlx::migrate::MigrateError> for KnowledgeError {
    fn from(value: sqlx::migrate::MigrateError) -> Self {
        KnowledgeError::MigrationError(value)
    }
}

pub type KnowledgeResult<T> = Result<T, KnowledgeError>;

#[derive(Clone)]
pub struct Knowledge {
    word_freq: WordFrequencyList,
    connection: Pool<Sqlite>
}

impl Knowledge {
    pub async fn new() -> Result<Self, KnowledgeError> {
        // Create the database.
        let connection = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::from_str("db.sqlite").unwrap() // TODO: error handling
                .create_if_missing(true)
            )
            .await?;

        // Run migrations.
        sqlx::migrate!().run(&connection).await?;

        Ok(Self {
            word_freq: WordFrequencyList::new(),
            connection
        })
    }
    
    fn tokenize_sentence_jumanpp(&self, sentence: &str) -> KnowledgeResult<Vec<String>> {
        let mut jumanpp = Command::new("jumanpp") // TEMP!!
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn().unwrap(); // TODO: Erro handling!

        if let Some(stdin) = jumanpp.stdin.as_mut().take() {
            stdin.write_all(sentence.as_bytes()).unwrap(); // TODO: error handling!
        } 

        match jumanpp.wait_with_output() {
            Ok(output) => {
                let data = String::from_utf8(output.stdout).unwrap(); // TODO: handle errors
                let mut words = Vec::new();

                // Parse the output and find the de-conjugated words.
                // Each line is a word (in order).
                // https://github.com/ku-nlp/jumanpp/blob/master/docs/output.md 
                // The third entry on each line is the dictionary form. That's what we want.
                // If a line start's with a '@' then that is an alias and we should maybe ignore
                // that and only take one version of the word.
                if output.status.success() {
                    for line in data.lines() {
                        // Ignore lines that start with '@'
                        if line.starts_with('@') {
                            continue;
                        }

                        // Split the line by spaces
                        let parts: Vec<&str> = line.split(" ").collect();

                        // Not exactly the best way to do this, but...
                        // There *should* be 12 space-separated fields, so expect that:
                        // Note: (this is <= 12 because the last field can sometimes be a quoted string that can contain spaces
                        // rather than actually parse this, bodge it by just expecting at least 12 fields. We aren't interested
                        // in the last fields anyway, so it's probably fine.) It might be a good idea to look
                        // at doing this properly at some point though. Maybe when I go through and sort out all of the error handling.
                        if parts.len() >= 12 {
                            let deconjugated = parts[2];

                            // Okay, so for some reason '\␣' is used to refer to a space.
                            // We uh don't want to include these.
                            if deconjugated == r"\␣" {
                                continue;
                            }

                            words.push(deconjugated.to_string());
                        }
                    }
                }

                Ok(words)
            },
            Err(e) => {
                // There was an error, maybe something wrong with the sentence, jumanpp wasn't installed.
                log::error!("Error calling jumanpp: {}", e);
                panic!(); // Just panic for now >.<
            }
        }
    } 

    pub async fn retokenize(&mut self) -> KnowledgeResult<()> {
        log::info!("Retokenizing sentences...");

        // First open a transaction.
        let mut tx = self.connection.begin().await?;

        // Now clear out the word_sentence table.
        log::info!("Clearing out word_sentence relationships...");
        sqlx::query("DELETE FROM word_sentence")
            .execute(&mut *tx).await?; // TODO: handle errors

        // Set the word tables count to 0 for everything
        log::info!("Setting all words count to 0...");
        sqlx::query("UPDATE words SET count = 0")
            .execute(&mut *tx).await?; // TODO: handle errors

        // Now go through each sentence and re-tokenize it
        log::info!("Iterating through all sentences and retokenizing...");
        let mut sentences_to_process = Vec::new();
        {
            let mut sentences_stream = sqlx::query("SELECT id, text, source FROM sentences")
                .fetch(&mut *tx);

            while let Some(row) = sentences_stream.try_next().await? { // TODO: error handling
                let sentence: String = row.try_get("text")?;
                let id: i64 = row.try_get("id")?;

                sentences_to_process.push((id, sentence));
            }
        }

        // Now the stream is closed...
        for (id, text) in sentences_to_process {
            // Tokenize
            let words = self.tokenize_sentence_jumanpp(text.as_str())?;

            // Re-add the sentences
            self.add_words_to_sentence(id, words, &mut *tx).await;
        }

        tx.commit().await?;

        log::info!("Finished re-tokenizing");

        // Done!
        Ok(())
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
    async fn get_words_in_sentence(&self, sentence_id: i64) -> KnowledgeResult<Vec<(i64, String)>> {
        let mut words = sqlx::query("
            SELECT word_id, sentence_id, words.text as word_text
            FROM word_sentence
                INNER JOIN words ON words.id = word_id
            WHERE sentence_id = ?")
            .bind(sentence_id)
            .fetch(&self.connection);

        let mut word_vec = Vec::new();
        while let Some(word_row) = words.try_next().await? { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id")?, word_row.try_get("word_text")?));
        }

        Ok(word_vec)
    }

    async fn get_words_in_sentence_that_need_reviewing(&self, sentence_id: i64) -> KnowledgeResult<Vec<(i64, String)>> {
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
        while let Some(word_row) = words.try_next().await? { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id")?, word_row.try_get("word_text")?));
        }

        Ok(word_vec)
    }

    async fn get_words_in_sentence_that_are_new(&self, sentence_id: i64) -> KnowledgeResult<Vec<(i64, String)>> {
        let mut words = sqlx::query("
            SELECT word_id, sentence_id, words.text as word_text, words.next_review_at
            FROM word_sentence
                INNER JOIN words ON words.id = word_id
            WHERE sentence_id = ?
                AND reviewed = FALSE")
            .bind(sentence_id)
            .fetch(&self.connection);

        let mut word_vec = Vec::new();
        while let Some(word_row) = words.try_next().await? { // TODO: error handling.
            word_vec.push((word_row.try_get("word_id")?, word_row.try_get("word_text")?));
        }

        Ok(word_vec)
    }

    pub async fn get_next_sentence_i_plus_one(&self) -> KnowledgeResult<IPlusOneSentenceData> {
        let end_of_day_time = self.get_end_of_day_time();
        let now_time = Local::now().fixed_offset();

        info!("Attempting to find a sentence to review...");

        // First we need to find sentences that are most optimal to meet the criteria of reviewing words that are expired.
        // So use a SUM and sub statement to sum the words that actually need reviewing today.
        // Find a the number of words that haven't been reviewed at all (new words).
        // Ignore any sentences with new words.
        // TODO: Maybe try and pick a random sentence that has the same amount of words that need reviewing? 
        match sqlx::query("
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
            HAVING
                words_that_are_new = 0
            ORDER BY
                words_that_need_reviewing DESC,
                words_that_are_new ASC,
                random()
            LIMIT 1
            ")
            .bind(end_of_day_time.to_rfc3339())
            .bind(now_time.to_rfc3339())
            .fetch_one(&self.connection)
            .await {
                
            Ok(row) => {
                // If there are no words that need reviewing in the selected sentence then we don't have any sentences to review!
                let words_that_need_reviewing: i64 = row.try_get("words_that_need_reviewing")?;
                let words_that_are_new: i64 = row.try_get("words_that_are_new")?;
                let sentence_text: String = row.try_get("sentence_text")?;
                info!("Found a sentence with {} words that need reviewing and {} new words. Sentence: {}", words_that_need_reviewing, words_that_are_new, sentence_text);

                // If there are words that need reviewing, do that!
                if words_that_need_reviewing > 0 {
                    // If we review this sentence we'll be reviewing some of the words we need to review. Return it!
                    let sentence_id = row.try_get("sentence_id")?;
                    let words_being_reviewed = self.get_words_in_sentence_that_need_reviewing(sentence_id).await?;
                    let words_that_are_new = self.get_words_in_sentence_that_are_new(sentence_id).await?;
                    let sentence_source = row.try_get("source")?;

                    return Ok(IPlusOneSentenceData {
                        sentence_id,
                        sentence_text,
                        sentence_source,
                        words_being_reviewed,
                        words_that_are_new
                    });
                }
            },
            Err(sqlx::Error::RowNotFound) => {
                // This ok, there might not be any sentences that have 0 new words.
                // Just continue and try the next query for new sentences and words.
                log::info!("Couldn't find a sentence that contains no new words!")
            },
            Err(e) => {
                return Err(KnowledgeError::from(e));
            }
        };

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
                average_new_word_count DESC,
                random()
            LIMIT 1")
            .fetch_one(&self.connection)
            .await {

            Ok(row) => {
                let words_that_are_new: i64 = row.try_get("words_that_are_new")?;
                let averag_word_count: f64 = row.try_get("average_new_word_count")?;
                info!("Found a sentence with {} new words with an average {} word count", words_that_are_new, averag_word_count);

                let sentence_id = row.try_get("sentence_id")?;
                let sentence_text = row.try_get("sentence_text")?;
                let words_being_reviewed = self.get_words_in_sentence_that_need_reviewing(sentence_id).await?;
                let words_that_are_new = self.get_words_in_sentence_that_are_new(sentence_id).await?;
                let sentence_source = row.try_get("source")?;

                Ok(IPlusOneSentenceData {
                    sentence_id,
                    sentence_text,
                    sentence_source,
                    words_being_reviewed,
                    words_that_are_new
                })
            },

            Err(sqlx::Error::RowNotFound) => {
                // Not entirely unexpected. It's possible there are no sentences with anything new to review.
                // TODO: This probably ought to be handled a bit better.
                // the page should probably not even show the review UI if there isn't anything to review.
                // It is a rather uncommon case however, especially if you have any decent amount of sentences in your database.
                // Probably will only appear to a user when they don't have any sentences in their database.
                Ok(IPlusOneSentenceData {
                    sentence_id: 0,
                    sentence_text: "No sentence with any new words and no words are scheduled for reviewing.".to_string(),
                    sentence_source: "".to_string(),
                    words_being_reviewed: vec![(0, "".to_string())],
                    words_that_are_new: vec![(0, "".to_string())]
                })
            },

            Err(e) => {
                // We weren't expecting this error!
                Err(KnowledgeError::from(e))
            }
        }
    }

    pub async fn review_sentence(&self, sentence_id: i64, response_quality: f64) -> KnowledgeResult<()> {
        // Find all the words in the sentence and then review them all!
        let words = self.get_words_in_sentence(sentence_id).await?;
        for (word_id, word_text) in words {
            self.review_word(word_id, response_quality).await?;
        }

        Ok(())
    }

    pub async fn get_review_info(&self) -> KnowledgeResult<ReviewInfoData> {
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
            .try_get(0)?;
        

        Ok(ReviewInfoData {
            reviews_remaining: review_count
        })
    }

    pub async fn review_word(&self, review_word_id: i64, response_quality: f64) -> KnowledgeResult<()> {
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
            
            Ok(row) => {
                // We found the word and it is a word that needs reviewing, or is a new word, so review it.
                // If this is a new word, use the default supermemo item.
                let reviewed: bool = row.try_get("reviewed")?;
                let mut sm = if !reviewed { 
                    SuperMemoItem::default()
                } else {
                    SuperMemoItem {
                        repitition: row.try_get("repitition")?,
                        e_factor: row.try_get("e_factor")?,
                        duration: Duration::seconds(row.try_get("review_duration")?)
                    }
                };

                // Calculate the values for the next review.
                sm = super_memo_2(sm, response_quality);
                let next_review_at = (Local::now().fixed_offset() + sm.duration).to_rfc3339();

                info!("Reviewing word id {}, updated review data: {:?}", review_word_id, &sm);

                // Store it.
                {
                    let mut tx = self.connection.begin().await?;
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
                        .execute(&mut *tx).await?;

                    tx.commit().await?;
                }

                Ok(())
            },

            Err(sqlx::Error::RowNotFound) => {
                // The word wasn't found. This should be because the word didn't need reviewing.
                log::info!("Word id {} doesn't need reviewing.", review_word_id);
                Ok(())
            },

            Err(e) => {
                Err(KnowledgeError::DatabaseError(e))
            }
        }
    }

    async fn add_sentence(&mut self, sentence: &str, source: &str) -> KnowledgeResult<()> {
        info!("Adding sentence {} from source {}", sentence, source);

        // Get the current datetime
        let now_time = Local::now().fixed_offset();

        // Tokenize the sentence to get the words.
        let words = self.tokenize_sentence_jumanpp(sentence)?;
        log::info!("Contains words: {:?}", words);

        // Start a database transaction.
        let mut tx = self.connection.begin().await?;

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
            self.add_words_to_sentence(sentence_id, words, &mut tx).await?;
        }

        // Commit to the transaction.
        tx.commit().await?; // TODO: error handling

        Ok(())
    }

    async fn add_words_to_sentence(&mut self, id: i64, words: Vec<String>, tx: &mut SqliteConnection) -> KnowledgeResult<()> {
        let now_time = Local::now().fixed_offset();

        log::info!("Adding words {:?}", words);

        // Let's go over the words.
        for word in &words {
            let freq = self.word_freq.get_word_freq(&word);

            // Insert into known words, or increment count if we already have it.
            sqlx::query(
                    "INSERT INTO words(count, frequency, text, date_added)
                        VALUES(1, ?, ?, ?)
                        ON CONFLICT(text) DO UPDATE SET count=count + 1;")
                    .bind(freq)
                    .bind(&word)
                    .bind(now_time.to_rfc3339())
                    .execute(&mut *tx).await?;

            // Create the word->sentence relationship.
            let word_id: i64 = sqlx::query(
                    "SELECT id, text
                        FROM words
                        WHERE text = ?")
                .bind(&word)
                .fetch_one(&mut *tx).await?
                .try_get("id")?;

            sqlx::query(
                    "INSERT OR IGNORE INTO word_sentence(word_id, sentence_id)
                        VALUES(?, ?);")
                .bind(word_id)
                .bind(id)
                .execute(&mut *tx).await?;
        }

        Ok(())
    }

    pub async fn add_text(&mut self, text: &str, source: &str) -> KnowledgeResult<i64> {
        let sentences = iterate_sentences(text);
        let sentences_count = sentences.len();
        for sentence in sentences {
            // Split the sentence into words and add that to the database.
            self.add_sentence(sentence.as_str(), source).await?;
        }

        Ok(sentences_count as i64)
    }
}