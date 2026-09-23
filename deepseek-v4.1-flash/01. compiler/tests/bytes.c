/* Byte-level checks: raw non-ASCII source bytes, and the fact that getchar
 * returns int so that 0xFF and 0x1A are ordinary data, not EOF.
 */
int printf(char *fmt, ...);
int getchar(void);

/* Comment carrying raw non-ASCII bytes: cafÃ© naÃ¯ve â‚¬ ÿþ end */

/* String literal with the same raw bytes, plus escape sequences.  The compiler
 * must copy every byte through unchanged. */
char *high = "cafÃ© Ã¼Ã¯ â‚¬ÿþ";
char *esc = "caf\xc3\xa9 \xff \xfe \x1a \x00end";

int main(void) {
    int i;
    int c;
    int n;
    int ff;
    int sub;
    int nul;
    int high_bytes;

    i = 0;
    while (high[i] != 0) {
        i++;
    }
    printf("len=%d\n", i);
    for (i = 0; i < 12; i++) {
        printf("%d ", (unsigned char)high[i]);
    }
    printf("\n");

    /* the escape-sequence literal must contain the same bytes */
    printf("esc0=%d esc1=%d esc2=%d\n", (unsigned char)esc[0], (unsigned char)esc[1], (unsigned char)esc[2]);
    printf("%d %d\n", (char)'\xff', (unsigned char)'\xff');
    printf("%d\n", (unsigned char)0xff);
    printf("%d %d %d\n", '\x1a', '\0', '\x7f');

    /* Count raw bytes on stdin.  0xFF must not be mistaken for EOF and 0x1A
     * must not terminate input. */
    n = 0;
    ff = 0;
    sub = 0;
    nul = 0;
    high_bytes = 0;
    while ((c = getchar()) != -1) {
        n++;
        if (c == 255) {
            ff++;
        }
        if (c == 26) {
            sub++;
        }
        if (c == 0) {
            nul++;
        }
        if (c >= 128) {
            high_bytes++;
        }
    }
    printf("n=%d ff=%d sub=%d nul=%d high=%d\n", n, ff, sub, nul, high_bytes);
    return 0;
}
