/* Calling-convention coverage: every argument count from 1 to 13, mixed
 * parameter widths, and calls nested inside argument lists.
 */
int printf(char *fmt, ...);

int f1(int a) { return a; }
int f2(int a, int b) { return a * 10 + b; }
int f3(int a, int b, int c) { return a * 100 + b * 10 + c; }
int f4(int a, int b, int c, int d) { return a * 1000 + b * 100 + c * 10 + d; }
int f5(int a, int b, int c, int d, int e) { return a + b * 2 + c * 3 + d * 4 + e * 5; }
int f6(int a, int b, int c, int d, int e, int g) { return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6; }
int f7(int a, int b, int c, int d, int e, int g, int h) { return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7; }
int f8(int a, int b, int c, int d, int e, int g, int h, int i) { return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7 + i * 8; }
int f9(int a, int b, int c, int d, int e, int g, int h, int i, int j) { return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7 + i * 8 + j * 9; }
int f11(int a, int b, int c, int d, int e, int g, int h, int i, int j, int k, int l) {
    return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7 + i * 8 + j * 9 + k * 10 + l * 11;
}
int f13(int a, int b, int c, int d, int e, int g, int h, int i, int j, int k, int l, int m, int n) {
    return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7 + i * 8 + j * 9 + k * 10 + l * 11 + m * 12 + n * 13;
}

int narrow(char a, unsigned char b, int c, char d, unsigned char e, int g, char h) {
    return a + b * 2 + c * 3 + d * 4 + e * 5 + g * 6 + h * 7;
}

int mixed(char *s, int a, char c, int *p, unsigned char u, int b) {
    return s[0] + a + c + *p + u + b;
}

int deep(int n) {
    if (n <= 0) {
        return 0;
    }
    return f13(n, n, n, n, n, n, n, n, n, n, n, n, n) + deep(n - 1);
}

int main(void) {
    int x = 7;
    char cs[3];
    cs[0] = 'A';
    cs[1] = 'B';
    cs[2] = 'C';

    printf("%d %d %d %d\n", f1(1), f2(1, 2), f3(1, 2, 3), f4(1, 2, 3, 4));
    printf("%d %d %d\n", f5(1, 2, 3, 4, 5), f6(1, 2, 3, 4, 5, 6), f7(1, 2, 3, 4, 5, 6, 7));
    printf("%d %d\n", f8(1, 2, 3, 4, 5, 6, 7, 8), f9(1, 2, 3, 4, 5, 6, 7, 8, 9));
    printf("%d %d\n", f11(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11),
                     f13(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13));

    // narrow and pointer parameters spread across register and stack slots
    printf("%d\n", narrow('a', 200, 3, 'd', 250, 6, 'h'));
    printf("%d\n", mixed(cs, 1, 'c', &x, 200, 6));
    printf("%d\n", mixed("z", 1, 'c', &x, 200, 6));

    // nested calls in argument lists, including stack arguments
    printf("%d\n", f7(f1(1), f2(1, 2), f3(1, 2, 3), f4(1, 2, 3, 4), f5(1, 2, 3, 4, 5), f6(1, 2, 3, 4, 5, 6), f1(7)));
    printf("%d\n", f8(f1(1), f2(1, 2), f3(1, 2, 3), f4(1, 2, 3, 4), f5(1, 2, 3, 4, 5), f6(1, 2, 3, 4, 5, 6), f1(7), f1(8)));
    printf("%d\n", f13(f1(1), 2, 3, 4, 5, 6, 7, 8, 9, 10, f11(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11), 12, f1(13)));

    // recursion through a wide call
    printf("%d\n", deep(6));

    // calls in conditions, loop steps and ternary branches
    {
        int i = 0;
        int s = 0;
        while (f1(i) < 5) {
            s = s + f2(i, 1);
            i = f1(i) + 1;
        }
        printf("%d %d\n", i, s);
        for (i = 0; i < 4; i = f1(i) + 1) {
            s = s + f3(1, 2, i);
        }
        printf("%d\n", s);
        printf("%d\n", f1(1) ? f2(2, 3) : f3(4, 5, 6));
        printf("%d\n", f1(0) ? f2(2, 3) : f3(4, 5, 6));
    }

    // an expression whose stack arguments contain further calls
    printf("%d\n", f5(f1(1) + f1(2), f2(1, f1(1)), f3(1, 1, f1(1)), f4(1, 1, 1, f1(1)), f1(5)));
    return 0;
}
