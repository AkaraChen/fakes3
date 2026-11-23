package io.github.akarachen.fakes3;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Just enough JSON for the control plane: objects, arrays, strings, numbers, booleans, null. */
final class Json {
  private final String s;
  private int i;

  private Json(String s) {
    this.s = s;
  }

  static Object parse(String text) {
    Json p = new Json(text);
    p.ws();
    Object v = p.value();
    p.ws();
    if (p.i != p.s.length()) {
      throw p.error("trailing characters");
    }
    return v;
  }

  @SuppressWarnings("unchecked")
  static Map<String, Object> parseObject(String text) {
    Object v = parse(text);
    if (!(v instanceof Map)) {
      throw new IllegalArgumentException("expected a JSON object: " + text);
    }
    return (Map<String, Object>) v;
  }

  static String quote(String v) {
    StringBuilder b = new StringBuilder(v.length() + 2).append('"');
    for (int k = 0; k < v.length(); k++) {
      char c = v.charAt(k);
      switch (c) {
        case '"':
          b.append("\\\"");
          break;
        case '\\':
          b.append("\\\\");
          break;
        case '\n':
          b.append("\\n");
          break;
        case '\r':
          b.append("\\r");
          break;
        case '\t':
          b.append("\\t");
          break;
        default:
          if (c < 0x20) {
            b.append(String.format("\\u%04x", (int) c));
          } else {
            b.append(c);
          }
      }
    }
    return b.append('"').toString();
  }

  private Object value() {
    if (i >= s.length()) {
      throw error("unexpected end");
    }
    char c = s.charAt(i);
    switch (c) {
      case '{':
        return object();
      case '[':
        return array();
      case '"':
        return string();
      case 't':
        return literal("true", Boolean.TRUE);
      case 'f':
        return literal("false", Boolean.FALSE);
      case 'n':
        return literal("null", null);
      default:
        if (c == '-' || (c >= '0' && c <= '9')) {
          return number();
        }
        throw error("unexpected character '" + c + "'");
    }
  }

  private Map<String, Object> object() {
    Map<String, Object> m = new LinkedHashMap<>();
    i++;
    ws();
    if (peek('}')) {
      i++;
      return m;
    }
    while (true) {
      ws();
      if (!peek('"')) {
        throw error("expected a key");
      }
      String k = string();
      ws();
      expect(':');
      ws();
      m.put(k, value());
      ws();
      if (peek(',')) {
        i++;
        continue;
      }
      expect('}');
      return m;
    }
  }

  private List<Object> array() {
    List<Object> l = new ArrayList<>();
    i++;
    ws();
    if (peek(']')) {
      i++;
      return l;
    }
    while (true) {
      ws();
      l.add(value());
      ws();
      if (peek(',')) {
        i++;
        continue;
      }
      expect(']');
      return l;
    }
  }

  private String string() {
    i++;
    StringBuilder b = new StringBuilder();
    while (true) {
      if (i >= s.length()) {
        throw error("unterminated string");
      }
      char c = s.charAt(i++);
      if (c == '"') {
        return b.toString();
      }
      if (c != '\\') {
        b.append(c);
        continue;
      }
      if (i >= s.length()) {
        throw error("unterminated string");
      }
      char e = s.charAt(i++);
      switch (e) {
        case '"':
        case '\\':
        case '/':
          b.append(e);
          break;
        case 'b':
          b.append('\b');
          break;
        case 'f':
          b.append('\f');
          break;
        case 'n':
          b.append('\n');
          break;
        case 'r':
          b.append('\r');
          break;
        case 't':
          b.append('\t');
          break;
        case 'u':
          if (i + 4 > s.length()) {
            throw error("bad \\u escape");
          }
          b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
          i += 4;
          break;
        default:
          throw error("bad escape");
      }
    }
  }

  private Object number() {
    int start = i;
    if (peek('-')) {
      i++;
    }
    boolean fraction = false;
    while (i < s.length()) {
      char c = s.charAt(i);
      if (c >= '0' && c <= '9') {
        i++;
      } else if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') {
        fraction = true;
        i++;
      } else {
        break;
      }
    }
    String n = s.substring(start, i);
    return fraction ? (Object) Double.parseDouble(n) : (Object) Long.parseLong(n);
  }

  private Object literal(String word, Object v) {
    if (!s.startsWith(word, i)) {
      throw error("unexpected token");
    }
    i += word.length();
    return v;
  }

  private void ws() {
    while (i < s.length() && Character.isWhitespace(s.charAt(i))) {
      i++;
    }
  }

  private boolean peek(char c) {
    return i < s.length() && s.charAt(i) == c;
  }

  private void expect(char c) {
    if (!peek(c)) {
      throw error("expected '" + c + "'");
    }
    i++;
  }

  private IllegalArgumentException error(String what) {
    return new IllegalArgumentException("invalid JSON at offset " + i + ": " + what);
  }
}
