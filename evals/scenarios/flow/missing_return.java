class Main {
    int ifOnly(boolean flag) {
        if (flag) {
            return 1;
        }
    /*@if_only*/}

    int loop(int n) {
        while (n > 0) {
            return n;
        }
    /*@loop*/}

    int switchNoDefault(int n) {
        switch (n) {
            case 1: return 1;
        }
    /*@switch_no_default*/}

    int tryCatch() {
        try {
            return 1;
        } catch (RuntimeException e) {
        }
    /*@try_catch*/}

    int ok1(boolean flag) {
        if (flag) return 1; else return 2;
    }

    int ok2() {
        while (true) {
        }
    }

    int ok3(int n) {
        switch (n) {
            case 1: return 1;
            default: throw new IllegalArgumentException();
        }
    }

    int ok4() {
        try {
            return 1;
        } finally {
            System.out.println();
        }
    }

    int ok5(int n) {
        for (;;) {
            if (n > 3) return n;
            n++;
        }
    }

    int ok6(Object o) {
        synchronized (o) {
            return 1;
        }
    }

    int ok7(boolean flag) {
        do {
            if (flag) return 1;
        } while (true);
    }
}
//? diag missing-return if_only
//? diag missing-return loop
//? diag missing-return switch_no_default
//? diag missing-return try_catch
//? diag exact
