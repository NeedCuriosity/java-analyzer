import java.util.List;

class Main {
    enum Color { RED, GREEN, BLUE }

    int count;

    void thrower() throws java.io.IOException {}

    int run(String s, int n, Object o, Color color) {
        int size = /*@new_method*/computeSize(s, n);
        /*@void_method*/log("x", size);
        /*@new_local*/total = n * 2;
        this.count = /*@new_field*/offset + 1;
        int limit = /*@constant*/MAX_SIZE;
        /*@unhandled*/thrower();
        String text = /*@cast_obj*/o;
        int small = /*@cast_num*/n * 2.5;
        /*@sw*/switch (color) {
            case RED -> size++;
        }
        int later;
        if (n > 0) {
            later = 1;
        }
        if (s /*@eq*/== "x" || s /*@neq*/!= text) {
            return /*@uninit*/later;
        }
        return size + limit + small;
    }

    static int helper() {
        return /*@static_method*/missing();
    }

    int oldSwitch(Color color) {
        /*@old_sw*/switch (color) {
            case RED:
                return 1;
        }
        return /*@expr_sw*/switch (color) {
            case GREEN -> 2;
            default -> 0;
        };
    }
}
//? assist create_method new_method ; has "private int computeSize(String s, int n) {" ; has "return 0;"
//? assist create_method void_method ; has "private void log(String string, int size) {"
//? assist create_method static_method ; has "private static int missing() {"
//? assist create_local_variable new_local ; has "int total = n * 2;"
//? assist create_field new_local ; has "private int total;"
//? assist create_field new_field ; has "private int offset;"
//? assist create_field constant ; has "private static final int MAX_SIZE = 0;"
//? assist add_throws unhandled ; has "int run(String s, int n, Object o, Color color) throws IOException {" ; has "import java.io.IOException;"
//? assist surround_with_try_catch unhandled ; has "try {\n            thrower();\n        } catch (IOException e) {\n            throw new RuntimeException(e);\n        }"
//? assist add_cast cast_obj ; has "String text = (String) o;"
//? assist add_cast cast_num ; has "int small = (int) (n * 2.5);"
//? assist add_missing_switch_cases sw ; has "case GREEN -> {" ; has "case BLUE -> {" ; lacks "case RED -> {"
//? assist add_missing_switch_cases old_sw ; has "case GREEN:\n                break;" ; has "case BLUE:"
//? assist add_missing_switch_cases expr_sw ; has "case RED -> throw new UnsupportedOperationException();" ; has "case BLUE -> throw"
//? assist initialize_variable uninit ; has "int later = 0;"
//? assist replace_with_equals eq ; has "s.equals(\"x\")"
//? assist replace_with_equals neq ; has "!s.equals(text)"
//? noassist create_method unhandled
//? noassist create_field cast_obj
//? noassist add_cast new_method
//? noassist add_throws new_method
//? noassist add_missing_switch_cases eq
