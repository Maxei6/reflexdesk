import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const FIXTURES_DIR = path.resolve("tests/fixtures");

test("Corpus integrity: speech audio corpus entries are valid and complete", () => {
  const corpusPath = path.join(FIXTURES_DIR, "corpora/speech_audio_corpus.jsonl");
  assert.ok(fs.existsSync(corpusPath), "speech_audio_corpus.jsonl must exist");

  const lines = fs.readFileSync(corpusPath, "utf8").trim().split("\n");
  assert.ok(lines.length >= 10, "corpus must have at least 10 entries");

  const languages = new Set();
  const accents = new Set();
  const speechRates = new Set();
  const noiseProfiles = new Set();

  for (const line of lines) {
    const entry = JSON.parse(line);
    assert.ok(entry.id, "entry must have id");
    assert.ok(entry.file, "entry must have file path");
    assert.ok(entry.duration_ms > 0, "entry must have positive duration");
    assert.equal(entry.sample_rate, 16000, "audio must be 16kHz standard");
    assert.ok(entry.language, "entry must specify language");
    assert.ok(entry.transcript, "entry must specify transcript");
    assert.ok(entry.risk_class, "entry must specify risk_class");

    languages.add(entry.language);
    accents.add(entry.accent);
    speechRates.add(entry.speech_rate);
    noiseProfiles.add(entry.noise_profile);

    // Verify referenced WAV file exists and has RIFF WAVE header
    const wavPath = path.resolve(entry.file);
    assert.ok(fs.existsSync(wavPath), `WAV file ${entry.file} must exist`);
    const header = Buffer.alloc(12);
    const fd = fs.openSync(wavPath, "r");
    fs.readSync(fd, header, 0, 12, 0);
    fs.closeSync(fd);

    assert.equal(header.toString("utf8", 0, 4), "RIFF", "Header must be RIFF");
    assert.equal(header.toString("utf8", 8, 12), "WAVE", "Header must be WAVE");
  }

  // Check diversity constraints
  assert.ok(languages.has("en"), "must contain English");
  assert.ok(languages.has("it"), "must contain Italian");
  assert.ok(languages.has("es"), "must contain Spanish");
  assert.ok(languages.has("fr"), "must contain French");
  assert.ok(languages.has("de"), "must contain German");
  assert.ok(speechRates.has("fast") && speechRates.has("slow") && speechRates.has("normal"), "must cover rate spectrum");
  assert.ok(noiseProfiles.has("clean") && noiseProfiles.has("noisy"), "must cover noise spectrum");
});

test("Corpus integrity: linguistic edge cases covers negations, corrections, ambiguity, destructiveness", () => {
  const corpusPath = path.join(FIXTURES_DIR, "corpora/linguistic_edge_cases.jsonl");
  assert.ok(fs.existsSync(corpusPath), "linguistic_edge_cases.jsonl must exist");

  const lines = fs.readFileSync(corpusPath, "utf8").trim().split("\n");
  assert.ok(lines.length >= 20, "corpus must have at least 20 linguistic cases");

  const categories = new Set();
  for (const line of lines) {
    const entry = JSON.parse(line);
    assert.ok(entry.id, "entry must have id");
    assert.ok(entry.category, "entry must have category");
    assert.ok(entry.input, "entry must have input text");
    assert.ok(entry.expected_outcome, "entry must specify expected_outcome");
    categories.add(entry.category);

    if (entry.category === "negation") {
      assert.equal(entry.expected_outcome, "deny", "negations must resolve to deny");
      assert.equal(entry.expected_tool, null, "negations must not execute any tool");
    }
    if (entry.category === "destructive") {
      assert.ok(
        entry.expected_outcome === "confirm" || entry.expected_outcome === "deny",
        "destructive actions must require confirm or deny"
      );
    }
  }

  assert.ok(categories.has("negation"), "must contain negation cases");
  assert.ok(categories.has("correction"), "must contain correction cases");
  assert.ok(categories.has("ambiguity"), "must contain ambiguity cases");
  assert.ok(categories.has("destructive"), "must contain destructive cases");
});

test("Corpus integrity: device transitions covers hardware and network events", () => {
  const corpusPath = path.join(FIXTURES_DIR, "corpora/device_transitions.jsonl");
  assert.ok(fs.existsSync(corpusPath), "device_transitions.jsonl must exist");

  const lines = fs.readFileSync(corpusPath, "utf8").trim().split("\n");
  assert.ok(lines.length >= 8, "corpus must have at least 8 device transitions");

  const eventTypes = new Set();
  for (const line of lines) {
    const entry = JSON.parse(line);
    assert.ok(entry.id, "entry must have id");
    assert.ok(entry.event, "entry must have event");
    eventTypes.add(entry.event);
  }

  assert.ok(eventTypes.has("mic_device_disconnected"), "must handle mic disconnect");
  assert.ok(eventTypes.has("mic_permission_denied"), "must handle mic permission denied");
  assert.ok(eventTypes.has("system_suspend_sleep"), "must handle system sleep");
  assert.ok(eventTypes.has("system_resume_wake"), "must handle system wake");
  assert.ok(eventTypes.has("network_loss_offline"), "must handle offline transition");
  assert.ok(eventTypes.has("network_restore_online"), "must handle online transition");
});
