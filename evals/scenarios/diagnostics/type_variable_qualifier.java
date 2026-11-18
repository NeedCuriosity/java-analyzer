class Main {
    static class Base {
        static int LIMIT = 3;

        static int count() {
            return 0;
        }
    }

    @SuppressWarnings("unchecked")
    static <T extends Enum> T nullSafe(Enum value, T defaultValue) {
        return value == null ? defaultValue : (T) T.valueOf(defaultValue.getClass(), value.toString());
    }

    static </*@b_decl*/B extends Base> int total() {
        return /*@b*/B.count() + B.LIMIT;
    }

    static <V extends Base> int broken() {
        return /*@missing*/W.count() + V./*@nope*/nope();
    }
}
//? def b -> b_decl
//? diag unresolved-name missing
//? diag unresolved-method nope
//? diag exact
