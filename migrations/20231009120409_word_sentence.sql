-- Add migration script here
CREATE TABLE IF NOT EXISTS word_sentence (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    sentence_id INTEGER NOT NULL REFERENCES sentences(id) ON DELETE CASCADE,
    PRIMARY KEY (word_id, sentence_id)
);
CREATE INDEX IF NOT EXISTS sentence_index ON word_sentence(sentence_id);
CREATE INDEX IF NOT EXISTS word_index ON word_sentence(word_id);