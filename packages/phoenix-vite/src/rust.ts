export interface RustToken {
  kind: "identifier" | "punctuation" | "string";
  value: string;
}

export function tokenizeRust(source: string): RustToken[] {
  const tokens: RustToken[] = [];
  let index = 0;
  while (index < source.length) {
    const character = source[index];
    if (/\s/.test(character)) {
      index += 1;
      continue;
    }
    if (source.startsWith("//", index)) {
      index = source.indexOf("\n", index + 2);
      if (index === -1) break;
      continue;
    }
    if (source.startsWith("/*", index)) {
      index = skipBlockComment(source, index);
      continue;
    }
    if (source[index] === "b" && source[index + 1] === '"') {
      const string = readRustString(source, index + 1);
      tokens.push({ kind: "string", value: string.value });
      index = string.end;
      continue;
    }
    if (source[index] === "b" && source[index + 1] === "r") {
      const rawByteString = readRawRustString(source, index + 1);
      if (rawByteString) {
        tokens.push({ kind: "string", value: rawByteString.value });
        index = rawByteString.end;
        continue;
      }
    }
    const rawString = readRawRustString(source, index);
    if (rawString) {
      tokens.push({ kind: "string", value: rawString.value });
      index = rawString.end;
      continue;
    }
    if (character === '"') {
      const string = readRustString(source, index);
      tokens.push({ kind: "string", value: string.value });
      index = string.end;
      continue;
    }
    if (character === "'" || (character === "b" && source[index + 1] === "'")) {
      const characterLiteral = readRustCharacter(source, character === "b" ? index + 1 : index);
      if (characterLiteral) {
        tokens.push({ kind: "string", value: characterLiteral.value });
        index = characterLiteral.end;
        continue;
      }
    }
    if (/[A-Za-z_]/.test(character)) {
      let end = index + 1;
      while (end < source.length && /[A-Za-z0-9_]/.test(source[end])) end += 1;
      tokens.push({ kind: "identifier", value: source.slice(index, end) });
      index = end;
      continue;
    }
    if (/[0-9]/.test(character)) {
      let end = index + 1;
      while (end < source.length && /[0-9_]/.test(source[end])) end += 1;
      tokens.push({ kind: "identifier", value: source.slice(index, end) });
      index = end;
      continue;
    }
    tokens.push({ kind: "punctuation", value: character });
    index += 1;
  }
  return tokens;
}

export function delimiterPairs(tokens: RustToken[], file: string): Map<number, number> {
  const pairs = new Map<number, number>();
  const stack: Array<{ index: number; value: string }> = [];
  const closing: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
  for (const [index, token] of tokens.entries()) {
    if (token.kind !== "punctuation") continue;
    if (["(", "[", "{"].includes(token.value)) {
      stack.push({ index, value: token.value });
    } else if (token.value in closing) {
      const opening = stack.pop();
      if (!opening || opening.value !== closing[token.value]) {
        throw new Error(`Phoenix could not parse Rust delimiters in ${file}`);
      }
      pairs.set(opening.index, index);
      pairs.set(index, opening.index);
    }
  }
  if (stack.length > 0) throw new Error(`Phoenix found an unclosed Rust delimiter in ${file}`);
  return pairs;
}

function skipBlockComment(source: string, start: number): number {
  let depth = 1;
  let index = start + 2;
  while (index < source.length && depth > 0) {
    if (source.startsWith("/*", index)) {
      depth += 1;
      index += 2;
    } else if (source.startsWith("*/", index)) {
      depth -= 1;
      index += 2;
    } else {
      index += 1;
    }
  }
  return index;
}

function readRawRustString(source: string, start: number): { end: number; value: string } | null {
  if (source[start] !== "r") return null;
  let quote = start + 1;
  while (source[quote] === "#") quote += 1;
  if (source[quote] !== '"') return null;
  const hashes = source.slice(start + 1, quote);
  const terminator = `"${hashes}`;
  const end = source.indexOf(terminator, quote + 1);
  if (end === -1) throw new Error("Phoenix found an unterminated Rust raw string");
  return { end: end + terminator.length, value: source.slice(quote + 1, end) };
}

function readRustString(source: string, start: number): { end: number; value: string } {
  let index = start + 1;
  let value = "";
  while (index < source.length) {
    const character = source[index];
    if (character === '"') return { end: index + 1, value };
    if (character !== "\\") {
      value += character;
      index += 1;
      continue;
    }
    const escaped = source[index + 1];
    const replacements: Record<string, string> = {
      "\\": "\\",
      '"': '"',
      n: "\n",
      r: "\r",
      t: "\t",
    };
    value += replacements[escaped] ?? escaped;
    index += 2;
  }
  throw new Error("Phoenix found an unterminated Rust string");
}

function readRustCharacter(
  source: string,
  start: number,
): { end: number; value: string } | null {
  let index = start + 1;
  let value = "";
  if (source[index] === "\\") {
    value = source.slice(index, index + 2);
    index += 2;
  } else if (source[index] && source[index] !== "\n" && source[index] !== "\r") {
    const point = source.codePointAt(index);
    if (point === undefined) return null;
    const character = String.fromCodePoint(point);
    value = character;
    index += character.length;
  }
  if (source[index] !== "'") return null;
  return { end: index + 1, value };
}
