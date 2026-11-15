class Main {
    enum Day { MON, TUE, SAT, SUN }

    String oldStyle(int code) {
        String name;
        /*@sw1*/switch (code) {
            case 1:
            case 7:
                name = "weekend";
                break;
            case 2:
                name = "monday";
                break;
            default:
                throw new IllegalArgumentException("bad " + code);
        }
        return name;
    }

    int arrows(Day day) {
        int hours = 0;
        System.out.println(hours);
        /*@sw2*/switch (day) {
            case MON, TUE -> hours = 8;
            case SAT -> {
                hours = 2;
            }
            default -> hours = 0;
        }
        return hours;
    }

    String returns(Day day) {
        /*@sw3*/switch (day) {
            case MON:
                return "start";
            case SUN:
                return "end";
            default:
                return "middle";
        }
    }

    void noDefault(Day day) {
        String s = "";
        /*@nosw1*/switch (day) {
            case MON -> s = "a";
            case TUE -> s = "b";
        }
        System.out.println(s);
    }

    void mixedTargets(int x) {
        int a = 0, b = 0;
        /*@nosw2*/switch (x) {
            case 1 -> a = 1;
            default -> b = 2;
        }
        System.out.println(a + b);
    }

    void sideEffects(int x) {
        int a = 0;
        /*@nosw3*/switch (x) {
            case 1:
                System.out.println("one");
                a = 1;
                break;
            default:
                a = 2;
        }
        System.out.println(a);
    }
}
//? assist switch_to_expression sw1 ; has "String name = switch (code) {\n            case 1, 7 -> \"weekend\";\n            case 2 -> \"monday\";\n            default -> throw new IllegalArgumentException(\"bad \" + code);\n        };\n        return name;"
//? assist switch_to_expression sw2 ; has "hours = switch (day) {\n            case MON, TUE -> 8;\n            case SAT -> 2;\n            default -> 0;\n        };"
//? assist switch_to_expression sw3 ; has "return switch (day) {\n            case MON -> \"start\";\n            case SUN -> \"end\";\n            default -> \"middle\";\n        };"
//? noassist switch_to_expression nosw1
//? noassist switch_to_expression nosw2
//? noassist switch_to_expression nosw3
