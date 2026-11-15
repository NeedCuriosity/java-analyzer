import java.util.List;
import java.util.function.Supplier;

class Main<T> {
    private int count;
    private static int total;

    private int /*@pure*/twice(int n) {
        return n * 2;
    }

    private int /*@static_use*/sum(List<Integer> items) {
        total += items.size();
        return helper(items.size()) + Math.max(1, 2);
    }

    private static int helper(int n) {
        return n;
    }

    private int usesField() {
        return count;
    }

    private int usesMethod() {
        return usesField();
    }

    private Main<T> usesThis() {
        return this;
    }

    private T usesTypeParam(Object o) {
        return null;
    }

    private Supplier<Integer> lambda() {
        return () -> count;
    }

    private Runnable anonymous() {
        return new Runnable() {
            public void run() {
            }
        };
    }

    public int notPrivate(int n) {
        return n;
    }

    @Deprecated
    private int annotated(int n) {
        return n;
    }

    private int qualified(Main<T> other) {
        return other.count;
    }

    private int referenced(int n) {
        return n + 1;
    }

    java.util.function.IntUnaryOperator ref() {
        return this::referenced;
    }

    private Object readResolve() {
        return null;
    }

    class Inner {
    }

    private Inner makeInner() {
        return new Inner();
    }
}
//? diag method-may-be-static pure
//? diag method-may-be-static static_use
//? diag only method-may-be-static
//? assist make_method_static pure ; has "private static int twice(int n) {"
