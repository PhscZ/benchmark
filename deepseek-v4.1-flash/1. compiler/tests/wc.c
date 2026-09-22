int printf(char *fmt, ...);
int getchar(void);
int putchar(int c);

int main(void) {
    int c;
    int n;
    int words;
    int lines;
    int inword;

    n = 0;
    words = 0;
    lines = 0;
    inword = 0;
    while ((c = getchar()) != -1) {
        n++;
        if (c == '\n') {
            lines++;
        }
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
            inword = 0;
        } else {
            if (!inword) {
                words++;
                inword = 1;
            }
        }
    }
    printf("%d %d %d\n", lines, words, n);
    putchar('.');
    putchar('\n');
    return 0;
}
