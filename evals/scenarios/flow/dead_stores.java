import java.util.List;

class Main {
    int compute() { return 1; }

    int overwritten() {
        int x = /*@d1*/0;
        x = compute();
        return x;
    }

    int branches(boolean c) {
        String s = /*@d2*/null;
        if (c) {
            s = "a";
        } else {
            s = "b";
        }
        return s.length();
    }

    int onlyOneBranch(boolean c) {
        String s = null;
        if (c) {
            s = "a";
        }
        return s == null ? 0 : s.length();
    }

    int loop(List<String> names) {
        int last = -1;
        for (String n : names) {
            if (n.isEmpty()) {
                continue;
            }
            last = n.length();
        }
        return last;
    }

    int loopCarried(int n) {
        int prev = 0;
        int cur = 1;
        for (int i = 0; i < n; i++) {
            int next = prev + cur;
            prev = cur;
            cur = next;
        }
        return cur;
    }

    int whileBreak(List<String> names) {
        int found = -1;
        int i = 0;
        while (true) {
            if (i >= names.size()) {
                break;
            }
            if (names.get(i).isEmpty()) {
                found = i;
                break;
            }
            i++;
        }
        return found;
    }

    String tryCatch() {
        String result = "default";
        try {
            result = String.valueOf(compute());
        } catch (RuntimeException e) {
            System.out.println(e);
        }
        return result;
    }

    int lastWrite(int a) {
        int y = a;
        System.out.println(y);
        y = /*@d3*/a + 1;
        return a;
    }

    int switchCase(int k) {
        int v;
        switch (k) {
            case 1:
                v = 10;
                break;
            case 2:
                v = /*@d5*/20;
            case 3:
                v = 30;
                break;
            default:
                v = 0;
        }
        return v;
    }

    int param(int p) {
        p = /*@d4*/5;
        p = compute();
        return p;
    }

    int lambda(List<String> names) {
        int limit = 3;
        names.removeIf(n -> n.length() > limit);
        return names.size();
    }

    int doWhile() {
        int tries = 0;
        int value;
        do {
            value = compute();
            tries++;
        } while (value < 0 && tries < 3);
        return value;
    }
}
//? diag unused-assignment d1
//? diag unused-assignment d2
//? diag unused-assignment d3
//? diag unused-assignment d4
//? diag unused-assignment d5
//? diag only unused-assignment
//? assist remove_unused d1 ; has "int x;\n        x = compute();"
//? assist remove_unused d3 ; lacks "y = a + 1"
