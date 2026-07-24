import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, extname, join, relative } from "node:path";

import { delimiterPairs, tokenizeRust, type RustToken } from "./rust.js";

export interface RustActionContract {
  input: string;
  output: string;
  route: string;
}

export interface GeneratedActionContract {
  input: string;
  output: string;
}

export interface ContractGeneration {
  actions: Map<string, GeneratedActionContract>;
  typeImports: string[];
}

type Direction = "input" | "output";
type ContractKind = "input" | "page" | "resource" | "shared";

interface RustAttribute {
  tokens: RustToken[];
}

interface ContractMetadata {
  kind: ContractKind;
  name?: string;
  namespace?: string;
  page?: string;
}

interface RustField {
  attributes: RustAttribute[];
  name: string;
  type: RustToken[];
}

interface RustVariant {
  attributes: RustAttribute[];
  name: string;
  shape: "unit" | "tuple" | "struct";
}

interface RustDeclaration {
  attributes: RustAttribute[];
  contract?: ContractMetadata;
  fields: RustField[];
  file: string;
  generic: boolean;
  kind: "enum" | "struct";
  name: string;
  variants: RustVariant[];
}

interface NeededContract {
  declaration: RustDeclaration;
  direction: Direction;
  exportName: string;
  typeRef: string;
}

interface EmittedField {
  name: string;
  optional: boolean;
  source: string;
  type: string;
}

const IGNORED_DIRECTORIES = new Set([
  ".git", ".idea", "dist", "node_modules", "public", "storage", "target",
]);

export function generateContractTypes(
  projectRoot: string,
  outputFile: string,
  actionContracts: RustActionContract[],
): ContractGeneration {
  const declarations = discoverDeclarations(projectRoot);
  const byName = new Map<string, RustDeclaration[]>();
  for (const declaration of declarations) {
    const matches = byName.get(declaration.name) ?? [];
    matches.push(declaration);
    byName.set(declaration.name, matches);
  }

  const roots: Array<{ declaration: RustDeclaration; direction: Direction }> = [];
  for (const declaration of declarations) {
    if (!declaration.contract) continue;
    roots.push({
      declaration,
      direction: declaration.contract.kind === "input" ? "input" : "output",
    });
  }
  for (const action of actionContracts) {
    roots.push({ declaration: resolveDeclaration(action.input, byName), direction: "input" });
    roots.push({ declaration: resolveDeclaration(action.output, byName), direction: "output" });
  }

  const directionsByDeclaration = new Map<RustDeclaration, Set<Direction>>();
  for (const root of roots) {
    const directions = directionsByDeclaration.get(root.declaration) ?? new Set<Direction>();
    directions.add(root.direction);
    directionsByDeclaration.set(root.declaration, directions);
  }

  const needed = new Map<string, NeededContract>();
  const queue = [...roots];
  while (queue.length > 0) {
    const current = queue.shift()!;
    const key = contractKey(current.declaration, current.direction);
    if (needed.has(key)) continue;
    const bothDirections = directionsByDeclaration.get(current.declaration)?.size === 2;
    const baseName = current.declaration.contract?.name ?? current.declaration.name;
    const exportName = bothDirections
      ? `${baseName}${current.direction === "input" ? "Input" : "Output"}`
      : baseName;
    const namespace = current.declaration.contract?.namespace;
    const typeRef = namespace ? `${namespace}.${exportName}` : exportName;
    needed.set(key, {
      declaration: current.declaration,
      direction: current.direction,
      exportName,
      typeRef,
    });
    enqueueReferencedTypes(current.declaration, current.direction, byName, queue);
  }

  const actionTypes = new Map<string, GeneratedActionContract>();
  for (const action of actionContracts) {
    const input = resolveDeclaration(action.input, byName);
    const output = resolveDeclaration(action.output, byName);
    actionTypes.set(action.route, {
      input: requiredNeeded(needed, input, "input").typeRef,
      output: requiredNeeded(needed, output, "output").typeRef,
    });
  }

  validateExportIdentities(needed.values());
  const module = contractModule(needed, byName);
  mkdirSync(dirname(outputFile), { recursive: true });
  if (!existsSync(outputFile) || readFileSync(outputFile, "utf8") !== module) {
    writeFileSync(outputFile, module);
  }

  return {
    actions: actionTypes,
    typeImports: [...new Set([...actionTypes.values()].flatMap(({ input, output }) => (
      [input.split(".")[0], output.split(".")[0]]
    )))].sort(),
  };
}

function discoverDeclarations(projectRoot: string): RustDeclaration[] {
  if (!existsSync(projectRoot)) return [];
  return walkRust(projectRoot).flatMap((file) => declarationsFromFile(file));
}

function walkRust(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isDirectory() && IGNORED_DIRECTORIES.has(entry.name)) return [];
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return walkRust(path);
    return extname(path) === ".rs" ? [path] : [];
  });
}

function declarationsFromFile(file: string): RustDeclaration[] {
  const tokens = tokenizeRust(readFileSync(file, "utf8"));
  const pairs = delimiterPairs(tokens, file);
  const declarations: RustDeclaration[] = [];
  for (let index = 0; index < tokens.length - 2; index += 1) {
    if (tokens[index].value !== "struct" && tokens[index].value !== "enum") continue;
    const kind = tokens[index].value as "struct" | "enum";
    const name = tokens[index + 1];
    if (name.kind !== "identifier") continue;
    const attributes = itemAttributes(tokens, pairs, index);
    const contract = contractMetadata(attributes, file);
    const bodyStart = findBodyStart(tokens, index + 2);
    if (bodyStart === undefined) continue;
    if (tokens[bodyStart].value !== "{") {
      if (contract) {
        throw new Error(
          `Phoenix contracts require named structs or enums (${name.value} in ${file})`,
        );
      }
      continue;
    }
    const bodyEnd = pairs.get(bodyStart);
    if (bodyEnd === undefined) throw new Error(`Phoenix found an unclosed ${kind} in ${file}`);
    declarations.push({
      attributes,
      contract,
      fields: kind === "struct" ? parseFields(tokens, pairs, bodyStart + 1, bodyEnd, file) : [],
      file,
      generic: tokens.slice(index + 2, bodyStart).some((token) => token.value === "<"),
      kind,
      name: name.value,
      variants: kind === "enum" ? parseVariants(tokens, pairs, bodyStart + 1, bodyEnd) : [],
    });
    index = bodyEnd;
  }
  return declarations;
}

function itemAttributes(
  tokens: RustToken[],
  pairs: Map<number, number>,
  itemIndex: number,
): RustAttribute[] {
  const attributes: RustAttribute[] = [];
  let cursor = itemIndex - 1;
  if (tokens[cursor]?.value === "pub") cursor -= 1;
  if (tokens[cursor]?.value === ")") {
    const opening = pairs.get(cursor);
    if (opening !== undefined && tokens[opening - 1]?.value === "pub") cursor = opening - 2;
  }
  while (tokens[cursor]?.value === "]") {
    const opening = pairs.get(cursor);
    if (opening === undefined || tokens[opening - 1]?.value !== "#") break;
    attributes.unshift({ tokens: tokens.slice(opening + 1, cursor) });
    cursor = opening - 2;
  }
  return attributes;
}

function findBodyStart(tokens: RustToken[], start: number): number | undefined {
  let angleDepth = 0;
  for (let index = start; index < tokens.length; index += 1) {
    if (tokens[index].value === "<") angleDepth += 1;
    if (tokens[index].value === ">") angleDepth -= 1;
    if (angleDepth === 0 && ["{", "(", ";"].includes(tokens[index].value)) return index;
  }
  return undefined;
}

function parseFields(
  tokens: RustToken[],
  pairs: Map<number, number>,
  start: number,
  end: number,
  file: string,
): RustField[] {
  return topLevelRanges(tokens, pairs, start, end).filter(([from, to]) => from < to).map(([from, to]) => {
    let cursor = from;
    const attributes: RustAttribute[] = [];
    while (tokens[cursor]?.value === "#" && tokens[cursor + 1]?.value === "[") {
      const close = pairs.get(cursor + 1);
      if (close === undefined || close >= to) throw new Error(`Invalid field attribute in ${file}`);
      attributes.push({ tokens: tokens.slice(cursor + 2, close) });
      cursor = close + 1;
    }
    if (tokens[cursor]?.value === "pub") {
      cursor += 1;
      if (tokens[cursor]?.value === "(") cursor = (pairs.get(cursor) ?? cursor) + 1;
    }
    const name = tokens[cursor];
    if (name?.kind !== "identifier" || tokens[cursor + 1]?.value !== ":") {
      throw new Error(`Phoenix contracts require named struct fields (${file})`);
    }
    return { attributes, name: name.value, type: tokens.slice(cursor + 2, to) };
  });
}

function parseVariants(
  tokens: RustToken[],
  pairs: Map<number, number>,
  start: number,
  end: number,
): RustVariant[] {
  return topLevelRanges(tokens, pairs, start, end).filter(([from, to]) => from < to).map(([from, to]) => {
    let cursor = from;
    const attributes: RustAttribute[] = [];
    while (tokens[cursor]?.value === "#" && tokens[cursor + 1]?.value === "[") {
      const close = pairs.get(cursor + 1);
      if (close === undefined || close >= to) break;
      attributes.push({ tokens: tokens.slice(cursor + 2, close) });
      cursor = close + 1;
    }
    const name = tokens[cursor]?.value ?? "";
    const shape = tokens[cursor + 1]?.value === "("
      ? "tuple"
      : tokens[cursor + 1]?.value === "{" ? "struct" : "unit";
    return { attributes, name, shape };
  });
}

function topLevelRanges(
  tokens: RustToken[],
  pairs: Map<number, number>,
  start: number,
  end: number,
): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  let rangeStart = start;
  let index = start;
  let angleDepth = 0;
  while (index < end) {
    if (tokens[index].value === "<") angleDepth += 1;
    if (tokens[index].value === ">") angleDepth -= 1;
    if (tokens[index].value === "," && angleDepth === 0) {
      ranges.push([rangeStart, index]);
      rangeStart = index + 1;
      index += 1;
      continue;
    }
    const pair = pairs.get(index);
    index = pair !== undefined && pair > index ? pair + 1 : index + 1;
  }
  ranges.push([rangeStart, end]);
  return ranges;
}

function contractMetadata(attributes: RustAttribute[], file: string): ContractMetadata | undefined {
  const attribute = attributes.find((candidate) => attributeName(candidate) === "contract");
  if (!attribute) return undefined;
  const open = attribute.tokens.findIndex((token) => token.value === "(");
  const args = open === -1 ? [] : attribute.tokens.slice(open + 1, -1);
  const kindToken = args.find((token) => token.kind === "identifier");
  const kind = kindToken?.value as ContractKind | undefined;
  if (!kind || !["input", "page", "resource", "shared"].includes(kind)) {
    throw new Error(`Phoenix contract kind must be input, page, resource, or shared (${file})`);
  }
  return {
    kind,
    name: attributeString(args, "name"),
    namespace: attributeString(args, "namespace"),
    page: kind === "page" ? attributeString(args, "page") : undefined,
  };
}

function attributeName(attribute: RustAttribute): string | undefined {
  const open = attribute.tokens.findIndex((token) => token.value === "(");
  return attribute.tokens.slice(0, open === -1 ? undefined : open)
    .filter((token) => token.kind === "identifier")
    .at(-1)?.value;
}

function attributeString(tokens: RustToken[], name: string): string | undefined {
  for (let index = 0; index < tokens.length - 2; index += 1) {
    if (tokens[index].value === name && tokens[index + 1].value === "="
      && tokens[index + 2].kind === "string") return tokens[index + 2].value;
  }
  return undefined;
}

function resolveDeclaration(
  type: string,
  byName: Map<string, RustDeclaration[]>,
): RustDeclaration {
  const name = rustTypeName(type);
  const matches = byName.get(name) ?? [];
  if (matches.length === 0) {
    throw new Error(`Phoenix could not find Rust contract type ${type}`);
  }
  if (matches.length > 1) {
    throw new Error(
      `Phoenix Rust contract type ${type} is ambiguous: ${matches.map((item) => item.file).join(", ")}`,
    );
  }
  return matches[0];
}

function rustTypeName(type: string): string {
  return type.replace(/\s/g, "").split("::").at(-1)!.replace(/<.*$/, "");
}

function enqueueReferencedTypes(
  declaration: RustDeclaration,
  direction: Direction,
  byName: Map<string, RustDeclaration[]>,
  queue: Array<{ declaration: RustDeclaration; direction: Direction }>,
): void {
  for (const field of declaration.fields) {
    for (const name of referencedTypeNames(field.type)) {
      const matches = byName.get(name);
      if (matches?.length === 1) queue.push({ declaration: matches[0], direction });
      if (matches && matches.length > 1) {
        throw new Error(`Phoenix nested contract type ${name} is ambiguous in ${declaration.file}`);
      }
    }
  }
}

function referencedTypeNames(tokens: RustToken[]): string[] {
  const builtins = new Set([
    "BTreeMap", "Box", "HashMap", "Option", "String", "Vec", "bool", "char",
    "f32", "f64", "i16", "i32", "i64", "i8", "i128", "isize", "str", "u16",
    "u32", "u64", "u8", "u128", "usize",
  ]);
  return [...new Set(tokens.filter((token) => token.kind === "identifier")
    .map((token) => token.value)
    .filter((name) => /^[A-Z]/.test(name) && !builtins.has(name)))];
}

function contractKey(declaration: RustDeclaration, direction: Direction): string {
  return `${declaration.file}:${declaration.name}:${direction}`;
}

function requiredNeeded(
  needed: Map<string, NeededContract>,
  declaration: RustDeclaration,
  direction: Direction,
): NeededContract {
  const contract = needed.get(contractKey(declaration, direction));
  if (!contract) throw new Error(`Phoenix internal contract resolution failed for ${declaration.name}`);
  return contract;
}

function validateExportIdentities(contracts: Iterable<NeededContract>): void {
  const seen = new Map<string, NeededContract>();
  for (const contract of contracts) {
    const previous = seen.get(contract.typeRef);
    if (previous && previous.declaration !== contract.declaration) {
      throw new Error(
        `Phoenix TypeScript contract name ${contract.typeRef} is duplicated in `
        + `${previous.declaration.file} and ${contract.declaration.file}`,
      );
    }
    seen.set(contract.typeRef, contract);
  }
}

function contractModule(
  needed: Map<string, NeededContract>,
  byName: Map<string, RustDeclaration[]>,
): string {
  const contracts = [...needed.values()].sort((left, right) => left.typeRef.localeCompare(right.typeRef));
  const plain = contracts.filter((contract) => !contract.declaration.contract?.namespace);
  const namespaces = new Map<string, NeededContract[]>();
  for (const contract of contracts) {
    const namespace = contract.declaration.contract?.namespace;
    if (!namespace) continue;
    const values = namespaces.get(namespace) ?? [];
    values.push(contract);
    namespaces.set(namespace, values);
  }

  const lines = [
    "// This file is generated by @apizero/vite from Rust contracts. Do not edit.",
    "",
    "export type PhoenixFieldDescriptor = {",
    "  readonly name: string;",
    "  readonly type: string;",
    "  readonly required: boolean;",
    "};",
    "export type PhoenixFieldMap = Record<string, PhoenixFieldDescriptor>;",
  ];
  for (const contract of plain) lines.push("", ...emitContract(contract, needed, byName, ""));
  for (const [namespace, values] of [...namespaces.entries()].sort()) {
    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(namespace)) {
      throw new Error(`Phoenix contract namespace is not a TypeScript identifier: ${namespace}`);
    }
    lines.push("", `export namespace ${namespace} {`);
    for (const contract of values) {
      lines.push(...emitContract(contract, needed, byName, "  "));
    }
    lines.push("}");
  }

  const pages = contracts.filter((contract) => contract.declaration.contract?.kind === "page");
  const pageNames = new Set<string>();
  lines.push("", "export interface PhoenixPageProps {");
  for (const page of pages) {
    const name = page.declaration.contract?.page;
    if (!name) throw new Error(`Phoenix page contract ${page.typeRef} requires page = \"...\"`);
    if (pageNames.has(name)) throw new Error(`Phoenix page contract is duplicated: ${name}`);
    pageNames.add(name);
    lines.push(`  ${JSON.stringify(name)}: ${page.typeRef};`);
  }
  lines.push("}");

  const shared = contracts.filter((contract) => contract.declaration.contract?.kind === "shared");
  lines.push(
    "",
    `export type PhoenixSharedProps = ${shared.length === 0 ? "Record<string, never>" : shared.map((item) => item.typeRef).join(" & ")};`,
  );
  const content = `${lines.join("\n")}\n`;
  return `${content}\nexport const contractHash = ${JSON.stringify(fnv1a(content))} as const;\n`;
}

function emitContract(
  contract: NeededContract,
  needed: Map<string, NeededContract>,
  byName: Map<string, RustDeclaration[]>,
  indent: string,
): string[] {
  const declaration = contract.declaration;
  if (declaration.generic) {
    throw new Error(
      `Phoenix currently does not support generic contract ${declaration.name} (${declaration.file})`,
    );
  }
  validateSerdeOptions(declaration.attributes, "container", declaration.file);
  if (declaration.kind === "enum") {
    const renameAll = serdeString(declaration.attributes, "rename_all", contract.direction);
    const variants = declaration.variants.flatMap((variant) => {
      validateSerdeOptions(
        variant.attributes,
        "variant",
        `${declaration.file}:${declaration.name}.${variant.name}`,
      );
      if (serdeSkip(variant.attributes, contract.direction)) return [];
      if (variant.shape !== "unit") {
        throw new Error(
          `Phoenix currently requires unit variants for contract enum ${declaration.name} (${declaration.file})`,
        );
      }
      const name = serdeString(variant.attributes, "rename", contract.direction)
        ?? applyRenameAll(variant.name, renameAll);
      return contract.direction === "input"
        ? [name, ...serdeStrings(variant.attributes, "alias")]
        : [name];
    });
    const duplicate = firstDuplicate(variants);
    if (duplicate) {
      throw new Error(
        `Phoenix Serde enum value ${duplicate} is duplicated in ${declaration.name} (${declaration.file})`,
      );
    }
    return [
      `${indent}export type ${contract.exportName} = ${variants.map((variant) => JSON.stringify(variant)).join(" | ")};`,
    ];
  }

  const fields = resolvedFields(declaration, contract.direction, needed, byName, []);
  const lines = [`${indent}export interface ${contract.exportName} {`];
  for (const field of fields) {
    lines.push(`${indent}  ${JSON.stringify(field.name)}${field.optional ? "?" : ""}: ${field.type};`);
  }
  lines.push(`${indent}}`);
  if (declaration.contract?.kind === "input") {
    lines.push("", `${indent}export const ${contract.exportName}Fields = {`);
    for (const field of fields) {
      const type = fieldTypeLabel(field.type);
      lines.push(
        `${indent}  ${JSON.stringify(field.name)}: {`
        + ` name: ${JSON.stringify(field.name)},`
        + ` type: ${JSON.stringify(type)},`
        + ` required: ${!field.optional} },`,
      );
    }
    lines.push(`${indent}} as const;`);
  }
  return lines;
}

function fieldTypeLabel(tsType: string): string {
  const base = tsType.replace(/\s*\|\s*null\b/g, "").trim();
  if (base.includes("[]")) return "array";
  if (base.startsWith("Record<") || base.startsWith("{") || base.startsWith("[")) return "object";
  if (base === "string" || base === "number" || base === "boolean" || base === "unknown") return base;
  if (base.includes("|") || base.includes("&")) return "unknown";
  if (/^[A-Za-z_$][\w.$]*$/.test(base)) return "object";
  return "unknown";
}

function resolvedFields(
  declaration: RustDeclaration,
  direction: Direction,
  needed: Map<string, NeededContract>,
  byName: Map<string, RustDeclaration[]>,
  flattenStack: RustDeclaration[],
): EmittedField[] {
  if (flattenStack.includes(declaration)) {
    throw new Error(`Phoenix found recursive #[serde(flatten)] in ${declaration.file}`);
  }
  validateSerdeOptions(declaration.attributes, "container", declaration.file);
  const renameAll = serdeString(declaration.attributes, "rename_all", direction);
  const containerDefault = direction === "input" && serdeHasOption(declaration.attributes, "default");
  const fields: EmittedField[] = [];
  for (const field of declaration.fields) {
    validateSerdeOptions(
      field.attributes,
      "field",
      `${declaration.file}:${declaration.name}.${field.name}`,
    );
    if (serdeSkip(field.attributes, direction)) continue;
    if (direction === "output" && hasAttribute(field.attributes, "sensitive")) {
      throw new Error(`Phoenix sensitive field ${declaration.name}.${field.name} cannot enter an output contract`);
    }
    if (serdeFlag(field.attributes, "flatten")) {
      const outer = outerTypeName(field.type);
      const nestedName = referencedTypeNames(field.type).at(-1);
      const nested = nestedName ? byName.get(nestedName) : undefined;
      if (!nested || nested.length !== 1 || nested[0].kind !== "struct"
        || (outer !== nestedName && outer !== "Option" && outer !== "Box")) {
        throw new Error(`Phoenix could not resolve flattened field ${declaration.name}.${field.name}`);
      }
      const flattened = resolvedFields(
        nested[0],
        direction,
        needed,
        byName,
        [...flattenStack, declaration],
      );
      const optional = containerDefault || (direction === "input" && outer === "Option");
      fields.push(...flattened.map((nestedField) => ({
        ...nestedField,
        optional: nestedField.optional || optional,
      })));
      continue;
    }
    const name = serdeString(field.attributes, "rename", direction)
      ?? applyRenameAll(field.name, renameAll);
    const optional = direction === "input"
      ? containerDefault || outerTypeName(field.type) === "Option"
        || serdeHasOption(field.attributes, "default")
      : serdeString(field.attributes, "skip_serializing_if", direction) !== undefined;
    fields.push({
      name,
      optional,
      source: `${declaration.file}:${field.name}`,
      type: rustTypeToTs(field.type, direction, needed, byName),
    });
    if (direction === "input") {
      for (const alias of serdeStrings(field.attributes, "alias")) {
        fields.push({ name: alias, optional: true, source: `${declaration.file}:${field.name} alias`, type: "never" });
      }
    }
  }

  const seen = new Map<string, EmittedField>();
  for (const field of fields) {
    const previous = seen.get(field.name);
    if (previous) {
      throw new Error(
        `Phoenix Serde wire name ${field.name} collides between ${previous.source} and ${field.source}`,
      );
    }
    seen.set(field.name, field);
  }
  return fields.filter((field) => field.type !== "never");
}

function rustTypeToTs(
  rawTokens: RustToken[],
  direction: Direction,
  needed: Map<string, NeededContract>,
  byName: Map<string, RustDeclaration[]>,
): string {
  const tokens = trimTypeTokens(rawTokens);
  if (tokens[0]?.value === "&") return rustTypeToTs(tokens.slice(tokens[1]?.value === "'" ? 3 : 1), direction, needed, byName);
  if (tokens[0]?.value === "(") {
    const inner = tokens.slice(1, -1);
    return `[${splitTypeArguments(inner).map((part) => rustTypeToTs(part, direction, needed, byName)).join(", ")}]`;
  }
  if (tokens[0]?.value === "[") {
    const separator = tokens.findIndex((token) => token.value === ";");
    const inner = separator === -1 ? tokens.slice(1, -1) : tokens.slice(1, separator);
    return `${parenthesize(rustTypeToTs(inner, direction, needed, byName))}[]`;
  }

  const genericStart = tokens.findIndex((token) => token.value === "<");
  const pathTokens = genericStart === -1 ? tokens : tokens.slice(0, genericStart);
  const name = pathTokens.filter((token) => token.kind === "identifier").at(-1)?.value ?? "";
  const args = genericStart === -1 ? [] : splitTypeArguments(tokens.slice(genericStart + 1, -1));
  const primitive: Record<string, string> = {
    String: "string", str: "string", char: "string", bool: "boolean",
    f32: "number", f64: "number", i8: "number", i16: "number", i32: "number",
    u8: "number", u16: "number", u32: "number",
  };
  if (primitive[name]) return primitive[name];
  if (["i64", "u64", "i128", "u128", "isize", "usize"].includes(name)) {
    throw new Error(`Phoenix will not silently map potentially unsafe Rust integer ${name} to TypeScript number`);
  }
  if (name === "Value") return "unknown";
  if (["DateTime", "NaiveDate", "NaiveDateTime", "Uuid"].includes(name)) return "string";
  if (name === "Option") return `${rustTypeToTs(args[0] ?? [], direction, needed, byName)} | null`;
  if (["Vec", "Box"].includes(name)) {
    const inner = rustTypeToTs(args[0] ?? [], direction, needed, byName);
    return name === "Vec" ? `${parenthesize(inner)}[]` : inner;
  }
  if (["HashMap", "BTreeMap"].includes(name)) {
    const key = rustTypeToTs(args[0] ?? [], direction, needed, byName);
    if (key !== "string") throw new Error(`Phoenix TypeScript record keys must serialize as strings`);
    return `Record<string, ${rustTypeToTs(args[1] ?? [], direction, needed, byName)}>`;
  }

  const matches = byName.get(name) ?? [];
  if (matches.length !== 1) {
    throw new Error(`Phoenix could not resolve nested Rust contract type ${name || "<empty>"}`);
  }
  return requiredNeeded(needed, matches[0], direction).typeRef;
}

function trimTypeTokens(tokens: RustToken[]): RustToken[] {
  let start = 0;
  let end = tokens.length;
  while (tokens[start]?.value === " ") start += 1;
  while (tokens[end - 1]?.value === " ") end -= 1;
  return tokens.slice(start, end);
}

function splitTypeArguments(tokens: RustToken[]): RustToken[][] {
  const parts: RustToken[][] = [];
  let start = 0;
  let angle = 0;
  let round = 0;
  let square = 0;
  for (let index = 0; index < tokens.length; index += 1) {
    const value = tokens[index].value;
    if (value === "<") angle += 1;
    if (value === ">") angle -= 1;
    if (value === "(") round += 1;
    if (value === ")") round -= 1;
    if (value === "[") square += 1;
    if (value === "]") square -= 1;
    if (value === "," && angle === 0 && round === 0 && square === 0) {
      parts.push(tokens.slice(start, index));
      start = index + 1;
    }
  }
  if (start < tokens.length) parts.push(tokens.slice(start));
  return parts;
}

function outerTypeName(tokens: RustToken[]): string | undefined {
  const genericStart = tokens.findIndex((token) => token.value === "<");
  return tokens.slice(0, genericStart === -1 ? undefined : genericStart)
    .filter((token) => token.kind === "identifier").at(-1)?.value;
}

function parenthesize(type: string): string {
  return type.includes(" | ") || type.includes(" & ") ? `(${type})` : type;
}

function hasAttribute(attributes: RustAttribute[], name: string): boolean {
  return attributes.some((attribute) => attributeName(attribute) === name);
}

function serdeFlag(attributes: RustAttribute[], name: string): boolean {
  return serdeAttributes(attributes).some((attribute) => (
    attribute.tokens.some((token, index) => token.value === name
      && attribute.tokens[index + 1]?.value !== "=")
  ));
}

function serdeHasOption(attributes: RustAttribute[], name: string): boolean {
  return serdeAttributes(attributes).some((attribute) => serdeOptionNames(attribute).includes(name));
}

type SerdeAttributeTarget = "container" | "field" | "variant";

function validateSerdeOptions(
  attributes: RustAttribute[],
  target: SerdeAttributeTarget,
  source: string,
): void {
  const allowed: Record<SerdeAttributeTarget, Set<string>> = {
    container: new Set([
      "bound", "crate", "default", "deny_unknown_fields", "expecting", "rename", "rename_all",
    ]),
    field: new Set([
      "alias", "borrow", "bound", "default", "flatten", "rename", "skip",
      "skip_deserializing", "skip_serializing", "skip_serializing_if",
    ]),
    variant: new Set([
      "alias", "rename", "skip", "skip_deserializing", "skip_serializing",
    ]),
  };
  for (const attribute of serdeAttributes(attributes)) {
    for (const name of serdeOptionNames(attribute)) {
      if (!allowed[target].has(name)) {
        throw new Error(
          `Phoenix does not support #[serde(${name})] on this ${target} contract yet (${source})`,
        );
      }
    }
  }
}

function serdeOptionNames(attribute: RustAttribute): string[] {
  const open = attribute.tokens.findIndex((token) => token.value === "(");
  if (open === -1) return [];
  const names: string[] = [];
  let depth = 0;
  let expectingName = true;
  for (let index = open + 1; index < attribute.tokens.length - 1; index += 1) {
    const token = attribute.tokens[index];
    if (token.value === "(") {
      depth += 1;
      continue;
    }
    if (token.value === ")") {
      depth -= 1;
      continue;
    }
    if (depth === 0 && expectingName && token.kind === "identifier") {
      names.push(token.value);
      expectingName = false;
      continue;
    }
    if (depth === 0 && token.value === ",") expectingName = true;
  }
  return names;
}

function serdeSkip(attributes: RustAttribute[], direction: Direction): boolean {
  return serdeFlag(attributes, "skip")
    || serdeFlag(attributes, direction === "input" ? "skip_deserializing" : "skip_serializing");
}

function serdeString(
  attributes: RustAttribute[],
  name: string,
  direction: Direction,
): string | undefined {
  for (const attribute of serdeAttributes(attributes)) {
    const tokens = attribute.tokens;
    for (let index = 0; index < tokens.length; index += 1) {
      if (tokens[index].value !== name) continue;
      if (tokens[index + 1]?.value === "=" && tokens[index + 2]?.kind === "string") {
        return tokens[index + 2].value;
      }
      if (tokens[index + 1]?.value === "(") {
        const section = tokens.slice(index + 2);
        const directional = attributeString(section, direction === "input" ? "deserialize" : "serialize");
        if (directional) return directional;
      }
    }
  }
  return undefined;
}

function serdeStrings(attributes: RustAttribute[], name: string): string[] {
  const values: string[] = [];
  for (const attribute of serdeAttributes(attributes)) {
    for (let index = 0; index < attribute.tokens.length - 2; index += 1) {
      if (attribute.tokens[index].value === name && attribute.tokens[index + 1].value === "="
        && attribute.tokens[index + 2].kind === "string") values.push(attribute.tokens[index + 2].value);
    }
  }
  return values;
}

function serdeAttributes(attributes: RustAttribute[]): RustAttribute[] {
  return attributes.filter((attribute) => attributeName(attribute) === "serde");
}

function applyRenameAll(name: string, rule?: string): string {
  if (!rule) return name;
  const words = splitWords(name);
  switch (rule) {
    case "lowercase": return words.join("").toLowerCase();
    case "UPPERCASE": return words.join("").toUpperCase();
    case "PascalCase": return words.map(capitalize).join("");
    case "camelCase": return words[0].toLowerCase() + words.slice(1).map(capitalize).join("");
    case "snake_case": return words.map((word) => word.toLowerCase()).join("_");
    case "SCREAMING_SNAKE_CASE": return words.map((word) => word.toUpperCase()).join("_");
    case "kebab-case": return words.map((word) => word.toLowerCase()).join("-");
    case "SCREAMING-KEBAB-CASE": return words.map((word) => word.toUpperCase()).join("-");
    default: throw new Error(`Phoenix does not recognize Serde rename_all rule ${rule}`);
  }
}

function splitWords(value: string): string[] {
  return value.replace(/([a-z0-9])([A-Z])/g, "$1_$2").split(/[_-]/).filter(Boolean);
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1).toLowerCase();
}

function firstDuplicate(values: string[]): string | undefined {
  const seen = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) return value;
    seen.add(value);
  }
  return undefined;
}

function fnv1a(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `fnv1a-${(hash >>> 0).toString(16).padStart(8, "0")}`;
}
