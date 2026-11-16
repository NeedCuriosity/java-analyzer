//- /Demo.java
class Demo {
    public String listBrand(@Param(value = "pageNum", defaultValue = "1") Integer pageNum,
                            @Param(value = "pageSize", defaultValue = "3") Integer pageSize) {
      String brandList = load(pageNum, pageSize);
        return brandList;
      }

    String load(Integer pageNum,
            Integer pageSize) {
    return "" + pageNum + pageSize;
    }

    Demo(int first,
         int second) throws IllegalStateException,
            IllegalArgumentException {
            this.toString();
    }
}

@interface Param {
    String value();
    String defaultValue();
}
//- /Demo.java formatted
class Demo {
    public String listBrand(@Param(value = "pageNum", defaultValue = "1") Integer pageNum,
                            @Param(value = "pageSize", defaultValue = "3") Integer pageSize) {
        String brandList = load(pageNum, pageSize);
        return brandList;
    }

    String load(Integer pageNum,
            Integer pageSize) {
        return "" + pageNum + pageSize;
    }

    Demo(int first,
         int second) throws IllegalStateException,
            IllegalArgumentException {
        this.toString();
    }
}

@interface Param {
    String value();

    String defaultValue();
}
