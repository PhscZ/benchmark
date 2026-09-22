int printf(char *fmt, ...);

int many(int a, int b, int c, int d, int e, int f, int g, int h) {
    return a + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8;
}

int mixed(char a, int b, char *c, int d, int e, int f, char *g) {
    return a + b + c[0] + d + e + f + g[0];
}

int twelve(int a, int b, int c, int d, int e, int f, int g, int h, int i, int j, int k, int l) {
    return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6
         + g * 7 + h * 8 + i * 9 + j * 10 + k * 11 + l * 12;
}

int main(void) {
    int n;

    printf("%d %d %d %d %d %d %d %d\n", 1, 2, 3, 4, 5, 6, 7, 8);
    printf("%s %d %c %s %d\n", "one", 2, '3', "four", 5);
    printf("%d %d %d %d %d %d %d %d %d %d\n", 10, 20, 30, 40, 50, 60, 70, 80, 90, 100);
    printf("%c%c%c%c%c%c%c%c\n", 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h');

    // printf returns the number of characters written
    n = printf("abc");
    printf(" %d\n", n);
    n = printf("");
    printf("%d\n", n);

    printf("%d\n", many(1, 2, 3, 4, 5, 6, 7, 8));
    printf("%d\n", twelve(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12));
    printf("%d\n", mixed('A', 1, "B", 2, 3, 4, "C"));

    // a call whose arguments are themselves calls
    printf("%d\n", many(1, 2, 3, 4, 5, 6, 7, 8) + many(8, 7, 6, 5, 4, 3, 2, 1));
    printf("%d\n", many(twelve(1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1), 0, 0, 0, 0, 0, 0, 0));

    // deep nesting with wide argument lists
    printf("%d\n", many(many(1, 0, 0, 0, 0, 0, 0, 0),
                        many(0, 1, 0, 0, 0, 0, 0, 0),
                        many(0, 0, 1, 0, 0, 0, 0, 0),
                        many(0, 0, 0, 1, 0, 0, 0, 0),
                        many(0, 0, 0, 0, 1, 0, 0, 0),
                        many(0, 0, 0, 0, 0, 1, 0, 0),
                        many(0, 0, 0, 0, 0, 0, 1, 0),
                        many(0, 0, 0, 0, 0, 0, 0, 1)));

    // variadic tail narrower than int, and pointer arguments
    {
        char c = 'x';
        unsigned char u = 200;
        char *s = "tail";
        printf("%d %d %s %c\n", c, u, s, c);
    }

    // calls inside loop conditions and bodies
    n = 0;
    while (many(n, 1, 1, 1, 1, 1, 1, 1) < 20) {
        n++;
    }
    printf("%d\n", n);
    return 0;
}
