/* A hex dump utility: reads stdin, prints offset / hex / ASCII, like `xxd`.
 * Exercises printf width and padding, bitwise ops, arrays, and binary input.
 */
int printf(char *fmt, ...);
int getchar(void);

int main(void) {
    unsigned char buf[16];
    int n;
    int total;
    int i;
    int c;

    total = 0;
    for (;;) {
        n = 0;
        while (n < 16) {
            c = getchar();
            if (c == -1) {
                break;
            }
            buf[n] = (unsigned char)c;
            n++;
        }
        if (n == 0) {
            break;
        }
        printf("%08x  ", total);
        for (i = 0; i < 16; i++) {
            if (i < n) {
                printf("%02x ", buf[i]);
            } else {
                printf("   ");
            }
            if (i == 7) {
                printf(" ");
            }
        }
        printf(" |");
        for (i = 0; i < n; i++) {
            if (buf[i] >= 32 && buf[i] < 127) {
                printf("%c", buf[i]);
            } else {
                printf(".");
            }
        }
        printf("|\n");
        total += n;
    }
    printf("%08x\n", total);
    return 0;
}
