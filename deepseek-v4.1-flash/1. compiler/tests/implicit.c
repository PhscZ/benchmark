/* Calls to functions that are defined later in the file without a preceding
 * prototype (C89-style implicit declaration), and mutual recursion declared
 * only by use.
 */
int printf(char *fmt, ...);

int main(void) {
    printf("%d\n", later(3));
    printf("%d\n", later(later(2)));
    printf("%d %d\n", is_even2(10), is_odd2(10));
    printf("%d %d\n", is_even2(7), is_odd2(7));
    printf("%d\n", sum_to(10));
    printf("%d\n", chained(4));
    return 0;
}

int later(int x) {
    return x * 7;
}

int is_even2(int n) {
    if (n == 0) {
        return 1;
    }
    return is_odd2(n - 1);
}

int is_odd2(int n) {
    if (n == 0) {
        return 0;
    }
    return is_even2(n - 1);
}

int sum_to(int n) {
    if (n <= 0) {
        return 0;
    }
    return n + sum_to(n - 1);
}

int chained(int n) {
    if (n <= 0) {
        return 1;
    }
    return later(chained(n - 1));
}
