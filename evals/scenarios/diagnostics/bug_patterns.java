import java.util.List;

class Main {
    private Object lock = new Object();
    private final Object finalLock = new Object();
    private volatile int count;
    private volatile boolean done;
    private int plain;

    void exceptions(int x) {
        if (x < 0) {
            new /*@thr*/IllegalArgumentException("negative");
        }
        // 匿名子类 / 抛出的 / 赋值的：不报
        throw new IllegalStateException("x");
    }

    static final int HOUR = 3600 * 1000;
    static final long DAY = 24 * HOUR;

    RuntimeException pick(int code) {
        return switch (code) {
            case 1 -> new IllegalStateException("one");
            default -> new IllegalArgumentException("other");
        };
    }

    void keep() {
        RuntimeException e = new RuntimeException();
        new Thread(() -> {}).start();
        new StringBuilder();
        System.out.println(e);
    }

    void locks() {
        synchronized (/*@sync*/lock) {
            plain++;
        }
        synchronized (finalLock) {
            plain++;
        }
        synchronized (this) {
            plain++;
        }
    }

    void counters(int n) {
        /*@inc*/count++;
        /*@add*/count += n;
        done = true;
        plain++;
        synchronized (finalLock) {
            count++;
        }
    }

    synchronized void guarded() {
        count--;
    }

    @interface GuardedBy {
        String value();
    }

    @GuardedBy("this")
    void callerHoldsLock() {
        count++;
    }

    void explicitLock(java.util.concurrent.locks.Lock guard) {
        guard.lock();
        try {
            count++;
        } finally {
            guard.unlock();
        }
    }

    long millis(int days, int hours) {
        long ms = /*@mul*/days * 24 * 3600 * 1000;
        long ok = (long) days * 24;
        long constant = 24 * 3600 * 1000;
        int small = days * hours;
        long shifted;
        shifted = /*@shl*/1 << hours;
        System.out.println(ms + ok + constant + small + shifted);
        return /*@ret*/days * hours;
    }

    void varargs(String a, String b, String[] names) {
        String s = String.format("%s %s", /*@arr*/new Object[] {a, b});
        // 一个 null / 空数组 / 本身就是数组的实参：不报
        String t = String.format("%s", new Object[] {null});
        String u = String.format("x", new Object[] {});
        String v = String.format("%s", new Object[] {names});
        System.out.println(s + t + u + v);
    }

    void builders(String name, int age, StringBuilder sb) {
        sb./*@app*/append("name=" + name + ", age=" + age);
        // 最左边不是字符串：前面的 `+` 是数值加法
        sb.append(age + age + "x");
        sb.append(name);
    }

    void labels(int[][] grid) {
        /*@lbl*/unused:
        for (int[] row : grid) {
            System.out.println(row.length);
        }
        outer:
        for (int[] row : grid) {
            for (int cell : row) {
                if (cell == 0) {
                    continue outer;
                }
            }
        }
    }

    void branches(int x, List<String> items) {
        if (x > 0) {
            System.out.println("same");
        } /*@same*/else {
            System.out.println("same");
        }
        if (items.remove("a")) {
            System.out.println("same");
        } /*@same2*/else {
            System.out.println("same");
        }
        if (x == 1) {
            System.out.println("one");
        } else if (x == 2) {
            System.out.println("two");
        } else if (/*@dup*/x == 1) {
            System.out.println("again");
        } else {
            System.out.println("other");
        }
        if (x > 0) {
            System.out.println("pos");
        } else {
            System.out.println("neg");
        }
        if (items.remove("a")) {
            System.out.println("first");
        } else if (items.remove("a")) {
            System.out.println("second");
        }
    }
}

//? diag throwable-not-thrown thr
//? diag only throwable-not-thrown
//? diag sync-on-non-final-field sync
//? diag only sync-on-non-final-field
//? diag non-atomic-volatile inc
//? diag non-atomic-volatile add
//? diag only non-atomic-volatile
//? diag int-multiplication-to-long mul
//? diag int-multiplication-to-long shl
//? diag int-multiplication-to-long ret
//? diag only int-multiplication-to-long
//? diag redundant-array-creation arr
//? diag only redundant-array-creation
//? diag append-concatenation app
//? diag only append-concatenation
//? diag unused-label lbl
//? diag only unused-label
//? diag duplicate-branches same
//? diag duplicate-branches same2
//? diag only duplicate-branches
//? diag duplicate-condition dup
//? diag only duplicate-condition
//? assist fix_redundancy thr ; has "throw new IllegalArgumentException(\"negative\");"
//? assist fix_redundancy mul ; has "long ms = (long) days * 24 * 3600 * 1000;"
//? assist fix_redundancy shl ; has "shifted = 1L << hours;"
//? assist fix_redundancy ret ; has "return (long) days * hours;"
//? assist fix_redundancy arr ; has "String.format(\"%s %s\", a, b);"
//? assist fix_redundancy app ; has "sb.append(\"name=\").append(name).append(\", age=\").append(age);"
//? assist fix_redundancy lbl ; has "    void labels(int[][] grid) {\n        for (int[] row : grid) {"
//? assist fix_redundancy same ; has "    void branches(int x, List<String> items) {\n        System.out.println(\"same\");\n        if (items"
//? noassist fix_redundancy same2
