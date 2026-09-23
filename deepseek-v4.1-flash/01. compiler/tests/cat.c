/* Byte-exact copy of stdin to stdout: exercises getchar/putchar, EOF
 * detection and binary-safe streams (NUL, CR, 0x1A and high bytes).
 */
int getchar(void);
int putchar(int c);

int main(void) {
    int c;
    while ((c = getchar()) != -1) {
        putchar(c);
    }
    return 0;
}
