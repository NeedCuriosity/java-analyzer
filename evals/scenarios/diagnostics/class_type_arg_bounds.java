import java.util.List;

class BoundClassR2<T extends Number> {
    BoundClassR2</*@field*/String> bad = new BoundClassR2</*@ctor*/String>();
    BoundClassR2<Integer> good = new BoundClassR2<>();
    BoundClassR2<?> wild;
    BoundClassR2<? extends Integer> upper;
    List<BoundClassR2<Long>> nested;

    void f(BoundClassR2</*@param*/Object> p) {
        BoundClassR2</*@local*/CharSequence> local = null;
    }

    static <U> void generic(BoundClassR2<? extends Number> ok) {}
}
//? diag type-arg-out-of-bounds field
//? diag type-arg-out-of-bounds ctor
//? diag type-arg-out-of-bounds param
//? diag type-arg-out-of-bounds local
//? diag exact
