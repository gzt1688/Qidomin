/**
 * lunar-javascript 自带类型缺失，这里只声明本项目用到的那部分 API。
 * 只声明用到的部分，避免和真实实现产生"看起来有类型其实是猜的"的错觉。
 */
declare module "lunar-javascript" {
  export class Solar {
    static fromDate(date: Date): Solar;
    static fromYmd(year: number, month: number, day: number): Solar;
    getYear(): number;
    /** 1-12（不是 0 起） */
    getMonth(): number;
    getDay(): number;
    /** 0=周日 */
    getWeek(): number;
    /** 一 / 二 / … / 日 */
    getWeekInChinese(): string;
    getXingZuo(): string;
    getFestivals(): string[];
    getOtherFestivals(): string[];
    toYmd(): string;
    getLunar(): Lunar;
  }

  export class JieQi {
    getName(): string;
    getSolar(): Solar;
  }

  export class Lunar {
    toString(): string;
    getYearInChinese(): string;
    getMonthInChinese(): string;
    getDayInChinese(): string;
    getYearInGanZhi(): string;
    getMonthInGanZhi(): string;
    getDayInGanZhi(): string;
    getYearShengXiao(): string;
    getAnimal(): string;
    getSeason(): string;
    /** 当天是节气时返回节气名，否则返回空串 */
    getJieQi(): string;
    /** 节气名 → 该节气的公历日期（覆盖约一个回归年，跨年时含相邻年份的节气） */
    getJieQiTable(): Record<string, Solar>;
    getPrevJieQi(): JieQi;
    getNextJieQi(): JieQi;
    /** 宜 */
    getDayYi(): string[];
    /** 忌 */
    getDayJi(): string[];
    getDayJiShen(): string[];
    getDayXiongSha(): string[];
    /** 值神（如 司命） */
    getDayTianShen(): string;
    /** 黄道 / 黑道 */
    getDayTianShenType(): string;
    /** 吉 / 凶 */
    getDayTianShenLuck(): string;
    /** 建除十二神：建 / 除 / 满 / 平 / 定 / 执 / 破 / 危 / 成 / 收 / 开 / 闭 */
    getZhiXing(): string;
    getXiu(): string;
    getXiuLuck(): string;
    /** 冲，如 "(壬午)马" */
    getDayChongDesc(): string;
    getDaySha(): string;
    getDayPositionCaiDesc(): string;
    getDayPositionFuDesc(): string;
    getDayPositionXiDesc(): string;
    getDayPositionYangGuiDesc(): string;
    getDayPositionYinGuiDesc(): string;
    getPengZuGan(): string;
    getPengZuZhi(): string;
    getFestivals(): string[];
    getOtherFestivals(): string[];
  }
}
