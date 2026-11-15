import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

sealed interface Expr permits Num, Add, Neg {}
record Num(int value) implements Expr {}
record Add(Expr left, Expr right) implements Expr {}
record Neg(Expr inner) implements Expr {}

abstract class Base {
    protected int id;
    Base(int id) { this.id = id; }
    abstract String name();
    static int counter;
    static { counter = 1; }
}

class Outer extends Base implements Iterable<String> {
    private final List<String> items = new ArrayList<>();

    Outer() { super(1); }

    @Override String name() { return "outer" + id; }

    class Inner {
        int peek() { return items.size() + id + Outer.this.id; }
    }

    static class Nested {
        final int v;
        Nested(int v) { this.v = v; }
    }

    enum Mode {
        ON { @Override int weight() { return 1; } },
        OFF { @Override int weight() { return 0; } };
        abstract int weight();
    }

    interface Visitor<R> {
        R visit(Outer o);
        default R fallback() { return null; }
    }

    @Override
    public Iterator<String> iterator() {
        return new Iterator<>() {
            int pos = 0;
            @Override public boolean hasNext() { return pos < items.size(); }
            @Override public String next() { return items.get(pos++); }
        };
    }

    static int eval(Expr e) {
        return switch (e) {
            case Num n -> n.value();
            case Add(Expr l, Expr r) -> eval(l) + eval(r);
            case Neg neg -> -eval(neg.inner());
        };
    }

    int run(Object o, Mode mode) {
        Inner inner = new Inner();
        Nested nested = new Outer.Nested(2);
        Visitor<Integer> v = out -> out.items.size();
        int total = inner.peek() + nested.v + v.visit(this) + mode.weight();
        if (o instanceof String s && !s.isEmpty()) {
            total += s.length();
        } else if (o instanceof Num(int value)) {
            total += value;
        }
        for (String item : this) {
            total += item.length();
        }
        String text = """
            block
            """;
        return total + text.length() + eval(new Add(new Num(1), new Neg(new Num(2))));
    }
}
//? diag none
