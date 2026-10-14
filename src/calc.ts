/**
 * 便签的「算式自动计算」。
 *
 * 用户敲 `6+3=` 这种"算式 + 等号"时，自动把结果补在等号后面（`6+3=9`）。
 *
 * 规则刻意做得保守，避免误伤普通文字：
 * - **触发条件**：整行以 `=`（或全角 `＝`）结尾；
 * - 只计算**最后一个等号之后**的那一段，所以可以连着写：`6+3=9+1=10`；
 * - 那一段必须全由数字 / 运算符 / 括号 / 小数点组成，且**至少有一个二元运算符**
 *   —— 否则 `123=`、`电话010-1234=` 这类也会被算成数字，那不是想要的结果；
 * - 除零、结果过大、括号不配对等一律"不算"，保持原文不动。
 *
 * 解析器是自己写的（递归下降），**不使用 eval / new Function**：
 * 便签正文是用户随手敲的任意文本，绝不能给它任何执行代码的机会。
 */

/** 复用的引用型结果 */
export interface CalcHit {
  /** 把 [`CalcHit::result`] 插到这个下标（紧跟触发用的那个等号） */
  at: number;
  /** 计算结果（已格式化） */
  result: string;
}

/** 表达式长度上限：纯手输的算式不会有这么长，超了说明不是算式 */
const MAX_LEN = 200;
/** 结果上限：超过就不显示，避免出现 `2e+21` 这种没人看得懂的输出 */
const MAX_ABS = 1e15;

/**
 * 全角 / 输入法符号 → 半角。
 *
 * 只用于"试着算一算"，**绝不改用户写下的原文** —— 用户敲的是 `6＋3＝`，
 * 补完结果后正文里仍然是那个全角加号。
 */
function normalize(src: string): string {
  let out = "";
  for (const ch of src) {
    const c = ch.codePointAt(0) ?? 0;
    // 全角数字 ０-９
    if (c >= 0xff10 && c <= 0xff19) {
      out += String.fromCharCode(c - 0xff10 + 0x30);
      continue;
    }
    switch (ch) {
      case "＋":
        out += "+";
        break;
      case "－":
      case "﹣":
      case "−":
      case "—":
      case "–":
        out += "-";
        break;
      case "＊":
      case "×":
      case "✕":
      case "✖":
      case "·":
      case "⋅":
        out += "*";
        break;
      case "／":
      case "÷":
        out += "/";
        break;
      case "＾":
        out += "^";
        break;
      case "（":
        out += "(";
        break;
      case "）":
        out += ")";
        break;
      case "，":
        out += ",";
        break;
      case "．":
      case "。":
        out += ".";
        break;
      case "\u3000":
        out += " ";
        break;
      default:
        out += ch;
    }
  }
  return out;
}

interface Parsed {
  v: number;
  /** 二元运算符的个数：0 个说明它只是"一个数"，不该被当成算式 */
  ops: number;
}

/**
 * 递归下降求值。语法：
 * ```
 * expr    := term (('+' | '-') term)*
 * term    := unary (('*' | '/') unary)*
 * unary   := ('+' | '-') unary | power
 * power   := atom ('^' power)?          // 右结合：2^3^2 = 2^9
 * atom    := number | '(' expr ')'
 * ```
 * 语法不合法（如 `1+`、括号不配对、除零）返回 `null`。
 */
function parse(s: string): Parsed | null {
  let i = 0;
  let ops = 0;
  /** 中途语法错误标记：省得每层都写一套错误传播 */
  let bad = false;

  const ws = () => {
    while (i < s.length && (s[i] === " " || s[i] === "\t")) i++;
  };

  const num = (): number | null => {
    ws();
    const start = i;
    let digit = false;
    let dot = false;
    while (i < s.length) {
      const c = s[i];
      if (c >= "0" && c <= "9") {
        i++;
        digit = true;
        continue;
      }
      if (c === "." && !dot) {
        dot = true;
        i++;
        continue;
      }
      // 千分位：逗号后面必须正好再跟 3 位数字，且不能出现在小数点之后
      if (c === "," && digit && !dot) {
        const grp = s.slice(i + 1, i + 4);
        const after = s[i + 4] ?? "";
        if (/^\d{3}$/.test(grp) && !(after >= "0" && after <= "9")) {
          i++;
          continue;
        }
      }
      break;
    }
    if (!digit) return null;
    const v = Number(s.slice(start, i).replace(/,/g, ""));
    return Number.isFinite(v) ? v : null;
  };

  const atom = (): number | null => {
    ws();
    if (s[i] === "(") {
      i++;
      const v = expr();
      if (v === null) return null;
      ws();
      if (s[i] !== ")") {
        bad = true;
        return null;
      }
      i++;
      return v;
    }
    return num();
  };

  const power = (): number | null => {
    const base = atom();
    if (base === null) return null;
    ws();
    if (s[i] === "^") {
      i++;
      ops++;
      const e = power();
      if (e === null) {
        bad = true;
        return null;
      }
      return Math.pow(base, e);
    }
    return base;
  };

  const unary = (): number | null => {
    ws();
    if (s[i] === "-") {
      i++;
      const v = unary();
      return v === null ? null : -v;
    }
    if (s[i] === "+") {
      i++;
      return unary();
    }
    return power();
  };

  const term = (): number | null => {
    let v = unary();
    if (v === null) return null;
    for (;;) {
      ws();
      const c = s[i];
      if (c !== "*" && c !== "/") break;
      i++;
      ops++;
      const r = unary();
      if (r === null) {
        bad = true;
        return null;
      }
      if (c === "/") {
        if (r === 0) {
          bad = true; // 除零：不算，保持原文
          return null;
        }
        v = v / r;
      } else {
        v = v * r;
      }
    }
    return v;
  };

  const expr = (): number | null => {
    let v = term();
    if (v === null) return null;
    for (;;) {
      ws();
      const c = s[i];
      if (c !== "+" && c !== "-") break;
      i++;
      ops++;
      const r = term();
      if (r === null) {
        bad = true;
        return null;
      }
      v = c === "+" ? v + r : v - r;
    }
    return v;
  };

  const v = expr();
  if (v === null || bad) return null;
  ws();
  if (i !== s.length) return null; // 还有没吃掉的字符 → 不是合法算式
  return { v, ops };
}

/** 去掉小数末尾多余的 0（`1.500` → `1.5`，`2.000` → `2`） */
function trimZeros(s: string): string {
  if (!s.includes(".")) return s;
  return s.replace(/0+$/, "").replace(/\.$/, "");
}

/** 结果 → 显示文本：抹掉浮点尾差（0.1+0.2 → 0.3），不出现科学计数法 */
export function fmtResult(v: number): string {
  if (!Number.isFinite(v) || Math.abs(v) > MAX_ABS) return "";
  if (Number.isInteger(v)) return String(v);
  // 12 位有效数字足够抹掉二进制浮点尾巴，又不损失用户能感知的精度
  let s = String(Number(v.toPrecision(12)));
  if (/[eE]/.test(s)) s = trimZeros(v.toFixed(12));
  return s;
}

/**
 * 判断一段文本是不是"末尾等号 + 可算的算式"，是则给出结果。
 *
 * `text` 是整行原文；返回值里的 `at` 是插入点（紧跟那个等号）。
 */
export function evalExpression(src: string): number | null {
  const s = normalize(src);
  if (s.length === 0 || s.length > MAX_LEN) return null;
  // 先做字符白名单：含中文 / 字母 / 其它符号的，直接判定"这不是算式"
  if (!/^[0-9+\-*/^().,\s]+$/.test(s)) return null;
  const r = parse(s);
  if (!r || r.ops === 0) return null;
  if (!Number.isFinite(r.v) || Math.abs(r.v) > MAX_ABS) return null;
  return r.v;
}

export function calcTail(text: string): CalcHit | null {
  if (!text) return null;
  // 结尾允许有空白：`6+3= ` 也算，结果插在等号正后方（不是行尾）
  let end = text.length;
  while (end > 0 && (text[end - 1] === " " || text[end - 1] === "\t" || text[end - 1] === "\u3000")) {
    end--;
  }
  if (end === 0) return null;
  const eqPos = end - 1;
  const eq = text[eqPos];
  if (eq !== "=" && eq !== "＝") return null;
  // 只算最后一个等号之后那一段 → 天然支持 `6+3=9+1=10` 这种连续往下算
  const start = Math.max(text.lastIndexOf("=", eqPos - 1), text.lastIndexOf("＝", eqPos - 1)) + 1;
  const v = evalExpression(text.slice(start, eqPos));
  if (v === null) return null;
  const result = fmtResult(v);
  if (!result) return null;
  return { at: eqPos + 1, result };
}

/** 把结果补进原文（不动前面的任何字符） */
export function applyCalc(text: string, hit: CalcHit): string {
  return text.slice(0, hit.at) + hit.result + text.slice(hit.at);
}
