import assert from "node:assert/strict";
import test from "node:test";

import { BracketedPasteAutoSubmit } from "./paste.ts";

const START = "\u001b[200~";
const END = "\u001b[201~";

test("leaves ordinary keyboard input unchanged", () => {
  const autoSubmit = new BracketedPasteAutoSubmit();

  assert.equal(autoSubmit.rewrite("hello", () => true), "hello");
});

test("leaves a manual clipboard paste open for editing", () => {
  const autoSubmit = new BracketedPasteAutoSubmit();
  const input = `${START}review this${END}`;

  assert.equal(autoSubmit.rewrite(input, () => false), input);
});

test("submits a completed Dictate clipboard transaction", () => {
  const autoSubmit = new BracketedPasteAutoSubmit();

  assert.equal(
    autoSubmit.rewrite(`${START}review this${END}`, () => true),
    `${START}review this${END}\r`,
  );
});

test("places submit before input following the paste", () => {
  const autoSubmit = new BracketedPasteAutoSubmit();

  assert.equal(
    autoSubmit.rewrite(`${START}review this${END}after`, () => true),
    `${START}review this${END}\rafter`,
  );
});

test("recognizes bracketed-paste tokens split across terminal chunks", () => {
  const autoSubmit = new BracketedPasteAutoSubmit();
  const chunks = ["\u001b[20", "0~review this\u001b[20", "1~"];
  const output = chunks.map((chunk) => autoSubmit.rewrite(chunk, () => true)).join("");

  assert.equal(output, `${START}review this${END}\r`);
});
