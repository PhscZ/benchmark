/* Tests for literals, escape sequences, comments and strings.
 * Both block comments and line comments appear here.
 */
int printf(char *fmt, ...);
int putchar(int c);

/* decimal and hexadecimal literals */
int dec = 1234;
int hex = 0x4D2;
int hexup = 0X4d2;
int zero = 0;
int oct = 010; /* octal is accepted too */

char *escapes = "tab:\there\nnl:[\n] cr:[\r] bs:[\\] q:[\"] a:[\'] nul-after-this\0hidden";

int main(void) {
    int i;
    char c;

    printf("%d %d %d %d %d\n", dec, hex, hexup, zero, oct);
    printf("%d %d %d\n", 0x7fffffff, 0xff, 0x10);
    printf("%d %d\n", 10, 0xa);
    printf("%d %d\n", 'A', 'z');
    printf("%d %d %d %d\n", '\n', '\r', '\t', '\0');
    printf("%d %d %d %d\n", '\\', '\'', '\"', '\a');
    printf("%d\n", '\x41');
    printf("%d\n", '\101');

    // string literal content, one byte at a time
    i = 0;
    while (escapes[i] != 0) {
        i++;
    }
    printf("len=%d\n", i);
    printf("%c%c%c%c\n", escapes[0], escapes[1], escapes[2], escapes[3]);

    // escapes render as the right control bytes
    c = '\n';
    printf("%d %d %d\n", c, '\t', '\r');
    printf("a\tb\n");
    printf("quote:\" backslash:\\ tick:\'\n");

    // hex literal arithmetic
    printf("%d %d %d\n", 0x10 + 0x20, 0xff & 0x0f, 0xff >> 4);

    // string literals are null terminated
    {
        char *s = "hello";
        int n = 0;
        while (s[n]) {
            n++;
        }
        printf("%d\n", n);
        printf("%c\n", s[n]);
        printf("%d\n", s[n]);
    }

    // adjacent use of the same literal
    {
        char *p = "abc";
        char *q = "abc";
        printf("%s%s\n", p, q);
        printf("%d %d %d\n", p[0], p[1], p[2]);
    }

    // empty string
    {
        char *e = "";
        printf("[%s] %d\n", e, e[0]);
    }

    // char array initialized from a literal, writable
    {
        char buf[6] = "hello";
        buf[0] = 'H';
        printf("%s %d\n", buf, sizeof(buf));
        printf("%d\n", buf[5]);
    }

    // partially initialized global char array
    {
        printf("%s %d\n", "0123456789", sizeof("0123456789"));
    }

    // comments do not disturb tokenization
    i = 1 /* inline */ + 2; // trailing
    printf("%d\n", i);

    // a comment marker inside a string is literal text
    printf("%s\n", "// not a comment /* nor this */");

    // putchar output
    putchar('o');
    putchar('k');
    putchar('\n');
    return 0;
}
