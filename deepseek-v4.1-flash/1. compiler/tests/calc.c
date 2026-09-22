/* A line-oriented integer calculator, like a small `bc`: reads lines such as
 * "12 * (3 + 4)" from stdin and prints one result per line.
 *
 * Exercises: a global char buffer, line input via getchar, mutual recursion
 * through an expression parser, char classification, and nested loops.
 */
int printf(char *fmt, ...);
int getchar(void);

char line[256];
int len;
int pos;

/* Reads one line; returns 0 at end of input. */
int read_line(void) {
    int c;
    len = 0;
    for (;;) {
        c = getchar();
        if (c == -1) {
            break;
        }
        if (len < 255) {
            line[len] = (char)c;
            len++;
        }
        if (c == '\n') {
            break;
        }
    }
    pos = 0;
    return len;
}

/* Current character, or 0 past the end of the line. */
int peekc(void) {
    if (pos >= len) {
        return 0;
    }
    return line[pos];
}

void advance(void) {
    if (pos < len) {
        pos++;
    }
}

int is_digit(int c) {
    return c >= '0' && c <= '9';
}

int is_space(int c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

void skip_space(void) {
    while (is_space(peekc())) {
        advance();
    }
}

int expr(void);

int primary(void) {
    int c;
    int v;

    skip_space();
    c = peekc();
    if (c == '(') {
        advance();
        v = expr();
        skip_space();
        if (peekc() == ')') {
            advance();
        }
        return v;
    }
    if (c == '-') {
        advance();
        return -primary();
    }
    if (is_digit(c)) {
        v = 0;
        while (is_digit(peekc())) {
            v = v * 10 + (peekc() - '0');
            advance();
        }
        return v;
    }
    return 0;
}

int term(void) {
    int v;
    int c;

    v = primary();
    for (;;) {
        skip_space();
        c = peekc();
        if (c == '*') {
            advance();
            v = v * primary();
        } else if (c == '/') {
            int d;
            advance();
            d = primary();
            if (d != 0) {
                v = v / d;
            } else {
                v = 0;
            }
        } else {
            return v;
        }
    }
}

int expr(void) {
    int v;
    int c;

    v = term();
    for (;;) {
        skip_space();
        c = peekc();
        if (c == '+') {
            advance();
            v = v + term();
        } else if (c == '-') {
            advance();
            v = v - term();
        } else {
            return v;
        }
    }
}

int main(void) {
    while (read_line() > 0) {
        printf("%d\n", expr());
    }
    return 0;
}
