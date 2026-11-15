import java.util.ArrayList;
import java.util.List;

class Main {
    private final List<String> names = new ArrayList<>();
    private int /*@init*/limit = 10;
    private String /*@ctor*/label;
    private int /*@this_ctor*/size;
    private static int /*@static_init*/counter;
    private int changed = 0;
    private int incremented;
    private int branch;
    private int twice;
    private int lambda;
    private int other;
    private int readFirst;
    private volatile int flag = 1;
    private transient int cache = 2;
    int packagePrivate = 3;
    private int a, b;
    @Deprecated private int annotated = 4;
    private int /*@initializer*/fromBlock;

    static {
        counter = 5;
    }

    {
        fromBlock = 6;
    }

    Main(String label, int size, boolean f) {
        this.label = label;
        this.size = size;
        incremented = 0;
        if (f) {
            branch = 1;
        } else {
            branch = 2;
        }
        twice = 1;
        twice = 2;
        Runnable r = () -> lambda = 3;
        other = 4;
        int seen = readFirst;
        readFirst = 5;
    }

    Main() {
        this("x", 0, false);
    }

    void mutate(Main m) {
        changed = 1;
        incremented++;
        m.other = 5;
    }

    int use() {
        return limit + size + counter + label.length() + branch + twice + lambda + other
            + readFirst + flag + cache + packagePrivate + a + b + annotated + fromBlock + names.size();
    }
}

class NoConstructor {
    private int /*@never_assigned*/never;
    private int assignedInMethod;

    void set() {
        assignedInMethod = 1;
    }

    int get() {
        return never + assignedInMethod;
    }
}

class Fix {
    private static String /*@fix*/NAME = "x";

    String name() { return NAME; }
}
//? diag field-may-be-final init
//? diag field-may-be-final ctor
//? diag field-may-be-final this_ctor
//? diag field-may-be-final static_init
//? diag field-may-be-final initializer
//? diag field-may-be-final fix
//? diag only field-may-be-final
//? assist simplify fix ; has "private static final String NAME = \"x\";"
