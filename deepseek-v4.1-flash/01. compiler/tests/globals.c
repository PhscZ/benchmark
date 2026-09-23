/* Statically initialized globals whose initializers are addresses, arrays of
 * pointers, and functions that take and return pointers.
 */
int printf(char *fmt, ...);

int value = 41;
int *value_ptr = &value;
char letters[] = "abcdef";
char *letters_ptr = letters;
char *names[4] = {"zero", "one", "two", "three"};
int table[4] = {10, 20, 30, 40};
int *table_rows[2] = {&table[0], &table[2]};
int nested[2][3] = {{1, 2, 3}, {4, 5, 6}};
char *greeting = "hi";
int (*unused_holder);
int *null_ptr;

/* Returns a pointer into its argument. */
char *skip_to(char *s, int c) {
    while (*s != 0) {
        if (*s == c) {
            return s;
        }
        s++;
    }
    return s;
}

char *advance(char *s, int n) {
    while (n > 0 && *s != 0) {
        s++;
        n--;
    }
    return s;
}

int *max_of(int *a, int n) {
    int *best;
    int i;
    if (n <= 0) {
        return a;
    }
    best = a;
    for (i = 1; i < n; i++) {
        if (a[i] > *best) {
            best = &a[i];
        }
    }
    return best;
}

char *middle(char *s) {
    int n;
    int i;
    n = 0;
    while (s[n] != 0) {
        n++;
    }
    return advance(s, n / 2);
}

int sum(int *a, int n) {
    int i;
    int t;
    t = 0;
    for (i = 0; i < n; i++) {
        t += a[i];
    }
    return t;
}

/* A pointer argument written through, with the result returned. */
char *fill(char *s, int c, int n) {
    int i;
    for (i = 0; i < n; i++) {
        s[i] = (char)c;
    }
    return s;
}

int main(void) {
    int i;
    char buf[8];

    printf("%d %d\n", value, *value_ptr);
    *value_ptr = 42;
    printf("%d\n", value);
    printf("%s %c\n", letters_ptr, *letters_ptr);
    printf("%d %d %d %d\n", names[0][0], names[1][1], names[2][2], names[3][0]);
    for (i = 0; i < 4; i++) {
        printf("%s ", names[i]);
    }
    printf("\n");
    printf("%d %d\n", *table_rows[0], *table_rows[1]);
    printf("%d %d %d\n", nested[0][2], nested[1][0], nested[1][2]);
    printf("%s\n", greeting);
    printf("%d\n", null_ptr == 0);
    null_ptr = table;
    printf("%d\n", *null_ptr);
    printf("%d\n", unused_holder == 0);

    printf("%s\n", skip_to(letters, 'd'));
    printf("%s\n", skip_to(letters, 'z'));
    printf("%s\n", advance(letters, 3));
    printf("%c\n", *middle(letters));
    printf("%c\n", *middle("abcde"));
    printf("%d\n", *max_of(table, 4));
    printf("%d\n", sum(table, 4));
    printf("%d\n", sum(table_rows[0], 2));

    {
        char *r = fill(buf, 'x', 4);
        buf[4] = 0;
        printf("%s %s\n", buf, r);
        printf("%d\n", r[0]);
    }

    /* pointer returned by one call fed straight into another */
    printf("%c\n", *advance(skip_to(letters, 'b'), 2));
    printf("%d\n", *max_of(&table[1], 3));

    /* do/while with continue and break */
    {
        int n = 0;
        int seen = 0;
        do {
            n++;
            if (n % 2 == 0) {
                continue;
            }
            seen++;
            if (n > 7) {
                break;
            }
        } while (n < 20);
        printf("%d %d\n", n, seen);
    }

    /* pointer arithmetic across a global array, written through a local alias */
    {
        int *p = table;
        int *end = table + 4;
        while (p < end) {
            *p = *p + 1;
            p++;
        }
        for (i = 0; i < 4; i++) {
            printf("%d ", table[i]);
        }
        printf("\n");
        printf("%d\n", (int)(end - table));
    }
    return 0;
}
